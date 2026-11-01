# Authenticode signing with SignPath Foundation (research notes and runbook)

Research date: 2026-10-31. Scope: sign `grimalkin.exe`, `grimalkin.com`, the Inno Setup
installer and its uninstaller, for x64 and arm64, from GitHub Actions, for free, using the
[SignPath Foundation](https://signpath.org/) open source program instead of buying a
certificate. This is the second signing route next to [AWS KMS](SIGNING-AWS.md); the
repository variable `WINDOWS_SIGNING_PROVIDER` (`kms` | `signpath`) picks one.

"Verified" = read on the cited page (fetched on this date). "Not found" = searched, no
official statement located. Nothing here is guessed; where the pipeline depends on something
that could only be tested with a real SignPath account it says so under "Open questions".

## 1. What the program is

- SignPath Foundation provides open source projects with code signing certificates through
  SignPath.io. "For OSS projects, our services are free of charge."
  <https://signpath.org/>, <https://signpath.org/about>
- SignPath's own open source page says the tools are "available at no cost".
  <https://about.signpath.io/product/open-source> (redirects to
  <https://signpath.io/solutions/open-source-community>)
- **The certificate is issued to SignPath Foundation, which is the publisher** of the signed
  software ("The code signing certificate is issued to SignPath Foundation. This means that
  SignPath Foundation is the publisher of the OSS project"). The key lives on SignPath's HSM
  and is never given to the project. <https://signpath.org/terms>
- SignPath Foundation is currently operated by SignPath GmbH, the company behind SignPath.io.
  <https://signpath.org/about>

## 2. Eligibility (all from <https://signpath.org/terms>; the page says its code of conduct is "still in a draft version")

Conditions for a free subscription:

| Condition | Wording | grimalkin |
| --- | --- | --- |
| No malware | "must not contain malware or potentially unwanted programs" | ok |
| License | "OSI-approved Open Source license without commercial dual-licensing for all components" | MIT OR Apache-2.0 (both OSI-approved; a choice of two open licenses, not commercial dual-licensing). GitHub's API reports `NOASSERTION` for the repository license (see "Open questions") |
| No proprietary code | "may not contain any proprietary, non open-source component"; System Libraries are allowed; unsigned binaries of upstream OSS projects may be included in signed packages | ConPTY (Microsoft, MIT) is redistributed unmodified and is never submitted for signing |
| Maintained | "actively maintained" | yes |
| Released | "must already be released in the form that should be signed" | releases exist (unsigned `rc` builds) |
| Documented | functionality described on the download page or app store entry | README |

Extra conditions because the certificate is the Foundation's:

- Sign your own projects and binaries only (built from your own source, you maintain the
  build scripts). Upstream binaries may be included but not signed.
- No hacking tools. Respect privacy: software that sends user data to systems the user did not
  choose must describe it in a privacy policy, show it during installation and offer an
  opt-out. Announce system changes (the installer's PATH and context-menu entries are optional
  tasks). Provide uninstallation (it does).
- All team members use multi-factor authentication for SignPath and for GitHub.
- Roles: **authors** (may change the code without review), **reviewers** (every change by
  non-committers is reviewed) and **approvers** ("each signing request must be approved by a
  team member trusted by the entire team"). Terms: "Every release needs manual approval for
  signing."
- The project home page needs a **"Code signing policy"** section (that exact term) with the
  text "Free code signing provided by SignPath.io, certificate by SignPath Foundation", the
  team roles and members, and a privacy policy link or the sentence "This program will not
  transfer any information to other networked systems unless specifically requested by the
  user or the person installing or operating it". The README now has it (section "Code
  signing policy"); it states that it takes effect once the application is approved.
- SignPath configuration: all signed binaries must have product name and product version
  enforced with file metadata restrictions (product name = the project name, product
  version = the same value for every file of a build). The artifact configurations in
  `installer/windows/signpath/` do this.
- "Binary artifacts must be built from source code in a verifiable way" (origin
  verification, section 5).
- The Foundation may pause or terminate the subscription and revoke the certificate "without
  prior notice" if the Code of Conduct is violated.

**Popularity / size / age:** no threshold of any kind appears in the terms ("Not found").
The repository is young (created 2026-10-28) and has no stars; nothing on the page makes
that a criterion, but the Foundation also states it performs "initial verification of project
reputation and control", which is a judgment we cannot predict.

## 3. Application process and turnaround

- Apply through the form at <https://signpath.org/apply> ("Apply for a free SignPath.io
  subscription"). The form is an embedded HubSpot form that did not render in the page text
  I could fetch, so its fields are **not verified**.
- Turnaround: **Not found.** No official statement. A third-party guide
  (<https://zenn.dev/shm_7ec/articles/signpath-oss-code-signing>) says "a few days"; other
  search results only mention projects still waiting. Plan for days to weeks.
- Support: <https://signpath.io/support>, abuse reports and policy questions
  `support@signpath.io`.
- SignPath lists the accepted projects at <https://signpath.org/projects> (`bottom`,
  `cargo-nextest`, `Ditto` and `cap` are among them: other Rust and terminal-adjacent
  projects, evidence that this kind of project is accepted).

## 4. What the certificate looks like, and timestamps

SignPath does **not document the subject or issuer** of the Foundation certificate ("Not
found" in the docs and terms). I therefore inspected the embedded signature of a real,
current SignPath Foundation-signed release: `btm.exe` from `bottom` 0.14.9
(<https://github.com/ClementTsang/bottom/releases/tag/0.14.9>, listed on the Foundation's
project page), parsed with `openssl pkcs7 -print_certs` on 2026-10-31:

```
leaf    : C=US, ST=Delaware, L=Lewes, O=SignPath Foundation, CN=SignPath Foundation
          valid 2026-01-28 .. 2027-09-07, Code Signing EKU
issuer  : C=BE, O=GlobalSign nv-sa, CN=GlobalSign GCC R45 CodeSigning CA 2020
root    : C=BE, O=GlobalSign nv-sa, CN=GlobalSign Code Signing Root R45
timestamp: RFC 3161, DigiCert Trusted G4 TimeStamping RSA4096 SHA256 2025 CA1
```

- The project name does **not** appear in the certificate subject: Windows shows the publisher
  as "SignPath Foundation" for every project. (Project identity is in SignPath's records and
  in the file's own version resource.)
- The issuing CA has changed over time: a third-party page
  (<https://www.freefixer.com/library/publisher/SignPath%20Foundation>) lists the issuer
  "SSL.com Code Signing Intermediate CA RSA R1" for older files. So the issuer is **not a
  constant we can commit once and forget**, and SignPath makes no promise about it. That is
  why the pipeline pins the issuer through a repository variable (`WINDOWS_SIGNING_ISSUER`)
  rather than a hard-coded chain file, and why the leaf certificate (owned and rotated by
  SignPath) is not pinned.
- Expected values as Windows PowerShell renders them (confirm with the first signed file, see
  section 8):
  - `WINDOWS_SIGNING_PUBLISHER` = `CN=SignPath Foundation, O=SignPath Foundation, L=Lewes, S=Delaware, C=US`
  - `WINDOWS_SIGNING_ISSUER` = `CN=GlobalSign GCC R45 CodeSigning CA 2020, O=GlobalSign nv-sa, C=BE`
- **Timestamps are included:** "If you use the file-based signing method of SignPath,
  timestamps will be managed automatically"
  (<https://docs.signpath.io/crypto-providers>, section Timestamps); the `bottom` file above
  carries an RFC 3161 countersignature. `verify-signature.ps1` still requires a trusted
  timestamp on every file.

## 5. How signing requests work

- GitHub's action: `signpath/github-action-submit-signing-request`
  (<https://github.com/SignPath/github-action-submit-signing-request>, docs
  <https://docs.signpath.io/trusted-build-systems/github>). Latest release v3.0 (2026-09-10),
  commit `f6d04783b4569d051e0c80105fe66e82819d0092`, which the workflow pins.
- Flow: a job uploads the files with `actions/upload-artifact` (v4 or later), then the action
  is given the resulting `artifact-id`. The action creates a signing request, waits for it
  (including manual approval) and downloads the signed artifact.
- Inputs (from the action's `action.yml` and the docs): `api-token` (required),
  `organization-id` (required), `project-slug` (required), `signing-policy-slug` (required),
  `artifact-configuration-slug` (default: the project's default configuration),
  `github-artifact-id` (required), `wait-for-completion` (default `true`),
  `wait-for-completion-timeout-in-seconds` (default 600 - the approval wait counts, so the
  workflow raises it), `output-artifact-directory` (where the signed artifact is extracted;
  nothing is downloaded without it), `parameters` (values for user-defined parameters of the
  artifact configuration), `github-token` (defaults to the workflow token; needs
  `actions: read` and `contents: read`), `skip-decompress`, `connector-url`. Outputs:
  `signing-request-id`, `signing-request-web-url`, `signed-artifact-download-url`.
- **Origin verification** (required for the open source program,
  <https://docs.signpath.io/origin-verification>): SignPath checks the repository URL
  configured in the project, the branch named in the signing policy, the commit and the
  build job URL, all supplied by GitHub through the "GitHub.com" trusted build system
  connector, so the build script cannot forge them. Checks listed on
  <https://docs.signpath.io/trusted-build-systems/github>: the build was performed by a
  GitHub workflow; the artifact is stored as a GitHub workflow artifact before submission; and,
  for OSS projects, **all jobs of the workflow leading up to the signing request ran on
  GitHub-hosted runners** (ours do: `windows-latest`, `windows-11-arm`, `ubuntu-latest`).
  Policy evaluation works for up to 3 re-runs of a build.
- Trust is placed in the workflow run: the signing job downloads the build job's artifacts
  and re-uploads them, so SignPath attests "this run of this repository at this commit
  produced and submitted these files", not the individual compile job. The workflow file is
  part of the source SignPath tells you to review ("Source code reviews must include build
  scripts").
- Release signing policies need manual approval (terms) and, for OSS, trusted build system
  verification and origin verification are required settings of signing policies
  (<https://docs.signpath.io/projects>). Approvers get e-mail notifications.
- Typical policy pair: `test-signing` (self-signed certificate, untrusted by Windows, no
  approval) and `release-signing` (the Foundation's certificate, approval required, origin
  verification, optional "Allowed branch names").

### File types

`<pe-file>` signs `.exe .dll .acm .ax .cpl .drv .efi .mui .ocx .scr .sys .tsp`; there are also
MSI/MSM/MSP, CAB, CAT, MSIX/APPX, PowerShell and script files, NuGet `.nupkg`, JAR, VSIX,
RPM, DEB and more (<https://docs.signpath.io/artifact-configuration/reference>). Notes that
matter here:

- **`.com` is not in the PE extension list.** `grimalkin.com` is a PE executable (the console
  launcher), so the workflow submits it under the name `grimalkin-launcher.exe` and renames
  it back after signing, then verifies the signature on the `.com` file. An Authenticode
  signature does not depend on the file name.
- Several files are signed in one request with a `<zip-file>` root; the GitHub action
  uploads a zip anyway, so the root element of every artifact configuration is `<zip-file>`.
- SignPath can sign an MSI and its contents in one pass ("deep signing"). **Inno Setup
  installers are not mentioned in the docs** (see below); an Inno Setup `.exe` is just a PE
  file for SignPath.
- `authenticode-verify` can check existing signatures (for example Microsoft's on ConPTY) but
  the pipeline already does that itself (`check-conpty.ps1`) and ConPTY is never submitted.

### Inno Setup and the uninstaller: what exists, what we chose

- "Inno Setup": **Not found** anywhere on docs.signpath.io, signpath.org or the action's
  repository, so SignPath has no documented pattern for it.
- The usual Inno approach is `SignTool=` in `[Setup]`, which makes ISCC run a command for
  Setup and for the generated uninstaller (`SignedUninstaller`)
  (<https://jrsoftware.org/ishelp/topic_setup_signtool.htm>,
  <https://jrsoftware.org/ishelp/topic_setup_signeduninstaller.htm>). A hook that signs each
  file through SignPath would need SignPath to be callable synchronously from the ISCC
  process, and it is not for an open source project:
  - the GitHub action cannot run inside ISCC's child process, and it requires the files to be
    uploaded as a GitHub artifact first (checked by the connector);
  - the SignPath **Crypto Providers** (the KSP that lets `signtool.exe` sign through
    SignPath) are documented as "Available for Code Signing Gateway"
    (<https://docs.signpath.io/crypto-providers>), an edition that is not the open source
    one (the pages that list "Open Source Code Signing" as available are origin verification,
    trusted build systems, user-defined parameters and approvals);
  - the plain REST API (<https://docs.signpath.io/build-system-integration>) carries no
    origin verification, which the open source policies require.
  So **the SignTool hook is not an option** for this project. This is a finding from the
  documentation, not a measured failure: if SignPath tells you the Crypto Providers are
  enabled for your project, a hook becomes possible (one request per file, each approved).
- **Chosen: pre-signed uninstaller.** Inno has a documented mode for exactly this case: with
  `SignedUninstaller=yes` and **no** `SignTool`, the first compile writes the unsigned
  uninstaller to `SignedUninstallerDir\uninst-<Inno version>-<hash>.e32` (`.e64`; a PE file with an
  Inno-specific extension, observed in CI) and stops with
  "Signed uninstaller mode is enabled. Using an external code-signing tool, please attach
  your digital signature to the following executable file ... and compile again". When the
  file is signed, later compiles copy its signature into the installer (Inno verifies that
  the signed file is byte-identical to the unsigned one apart from the signature, and the file
  name contains a hash of the unsigned content). I read the compiler source for this
  (`jrsoftware/issrc`, `Compiler.SetupCompiler.pas`, procedure `SignSetupMemoryFile`) and the
  help page above. The unsigned uninstaller should not depend on the program files, so it can be produced
  before they are signed and signed together with them; `test-signpath-inno-flow.ps1` checks
  that on every CI run. The workflow submits it under an `.exe` name (SignPath's PE type does
  not list `.e32`) and restores the Inno name afterwards; the bytes are identical.

### The flow in this repository (two signing requests per release)

```
windows build (x64, arm64)         unsigned, no secrets
windows sign (signpath)
  1. ISCC stop-at-uninstaller      scripts/windows/build-installer.ps1 -SignPathGenerateUninstaller
  2. REQUEST 1  (approval)         x64|arm64: grimalkin.exe, grimalkin-launcher.exe (= grimalkin.com), uninst-*.exe
  3. verify (publisher + issuer + timestamp), put signed files in place, rename launcher back to .com
  4. ISCC final compile            build-installer.ps1 -SignPathSetup   (embeds the signed uninstaller; fails otherwise)
  5. REQUEST 2  (approval)         x64|arm64: grimalkin-setup-<arch>.exe
  6. verify, CI test twins, portable zips (verified again), upload
windows smoke (x64, arm64)         installs the released installer, verifies Setup, program files and unins000.exe
windows release assets
```

Rate limits and quotas: **Not found** (no number on the terms, the Foundation site or SignPath's
open source page; a software directory mentions "fair-use quotas" without figures). Two
requests with a few files each per release is small. Each request waits for a manual approval
(default policy), so a release needs you at SignPath twice. File size limits: not found.

Known limitations of this flow:

- Inno's help says Setup "may use temporary self-copies, which aren't signed if you sign Setup
  yourself after compilation". The released Setup is signed after compilation here, so those
  temporary copies are unsigned (the KMS route signs them through `SignTool=`). Whether that
  changes any prompt (for example the UAC dialog of an all-users install) is on the Windows
  acceptance checklist; the default per-user install has no UAC prompt.
- `scripts/windows/test-signpath-inno-flow.ps1` runs on every CI run for both architectures and
  proves the Inno behaviour the flow depends on (unsigned uninstaller independent of the
  payload, unsigned uninstaller refused, signed one embedded and present in the installed
  `unins000.exe`) with a throwaway self-signed certificate. It cannot prove SignPath's
  signature is accepted by Inno; the first real release does (the final compile fails
  closed if not).

## 6. Comparison with the AWS KMS route

| | AWS KMS + purchased certificate | SignPath Foundation |
| --- | --- | --- |
| Cost | KMS about $12/yr plus a certificate: SSL.com $129/yr, DigiCert about $439/yr, GlobalSign by quote (see SIGNING-AWS.md) | $0 |
| Key custody | Non-exportable RSA key in your AWS account; CI role may only `Sign`; you run CloudTrail and answer the CA's key-protection questions | Key on SignPath's HSM; you never hold it and cannot export or revoke it yourself |
| Who the publisher is | You (individual name for IV, your entity for OV) | "SignPath Foundation" for every project; the project is not in the certificate |
| Control | Yours: you can sign anything, any time | Foundation rules: only your own binaries, origin verification, manual approval for every release; can pause the subscription or revoke the certificate |
| Renewal | Yearly (certificates last at most 460 days): new CSR, replace `codesign-chain.pem`, maybe update the publisher variable | Nothing to do; SignPath rotates the leaf. The issuer or publisher string can change: you update `WINDOWS_SIGNING_ISSUER` if verification starts failing |
| Latency | Fully automatic on a tag | Application review first (unknown), then two manual approvals per release (the job waits, 2 h by default) |
| Setup effort | CloudFormation, CSR from KMS, CA vetting and attestation questions, OIDC role | Application, SignPath project/policies/connector, one secret and some variables |
| CI exposure | `id-token: write` on one job, AWS role, jsign jar | No cloud credentials and no `id-token`; the SignPath API token (submitter only) |
| SmartScreen reputation | Builds up for your certificate (see README) | Not verified. Reputation attaches to the signing identity, which here is shared by every Foundation project; whether that helps a new project is unknown |
| Dependence | Your account and your CA | SignPath's availability and the Foundation's continued acceptance |

Recommendation for this project: use SignPath. It is free, needs no key custody, and removes
the CA vetting step that is the hard part of the KMS route for an individual. Keep the KMS
route in the repository as the fallback: switching is one repository variable.

## 7. Owner steps

Do these in order. Steps 1-3 can start now; nothing in the repository changes behaviour until
step 11.

1. **Merge the PR that adds this document** so the README's "Code signing policy" section is
   on the home page, then check it names the right people (you are listed as the only
   author, reviewer and approver; adjust if the team grows) and that your GitHub and SignPath
   accounts use multi-factor authentication.
2. **Apply** at <https://signpath.org/apply>. Have ready: the repository URL, the license, a
   statement of what the software does, the download page (the GitHub releases page). Mention
   that you will build and sign from GitHub Actions on tag pushes.
3. Wait for the e-mail from SignPath. If it takes long, write to the support address on
   <https://signpath.io/support>.
4. **Log in to <https://app.signpath.io>.** The organization is created for you. Note the
   **Organization ID** (the signing policy details page shows every ID plus a code snippet).
5. **Trusted build systems:** add the predefined "GitHub.com" system to the organization and
   link it to the project (<https://docs.signpath.io/trusted-build-systems>). Installing the
   SignPath GitHub App is only needed for audit-log based pipeline policies; this project does
   not use them.
6. **Project:** slug `grimalkin`, repository URL `https://github.com/rlankenau/grimalkin`
   (must match what GitHub reports: origin verification compares it).
7. **Artifact configurations** (project > Artifact configurations > Add > Custom, paste the
   XML, check it in the editor, which validates against SignPath's schema; I could not run the
   XML through SignPath):
   - `binaries` from `installer/windows/signpath/artifact-configuration-binaries.xml`
   - `setup` from `installer/windows/signpath/artifact-configuration-setup.xml`
   Both take a required user-defined parameter `version` (the workflow passes the tag
   version) and enforce product name `Grimalkin`.
8. **Signing policies.** I could not see what the Foundation preconfigures for a new open source
   organization, so this lists the settings the documentation describes
   (<https://docs.signpath.io/projects>); follow the settings, not the names.
   - `release-signing`: purpose release; certificate: the SignPath Foundation one;
     submitters: the CI user from step 9; **use approval process** with you as approver (1
     approval); trusted build system verification and **Verify origin** on (required for open
     source). "Allowed branch names": the release workflow runs on a tag, and I found no
     documentation of how a tag ref is matched, so start without a branch restriction and ask
     SignPath support before adding one.
   - `test-signing` (optional, self-signed certificate): useful to try the SignPath side with
     a manually uploaded zip of the `dist-x64` artifact. The pipeline cannot use it for a
     release: its signatures are not trusted, so `verify-signature.ps1` would (correctly) fail.
9. **CI user and token:** create a dedicated CI user, make it a submitter on `release-signing`
   (and `test-signing`), create an API token for it (<https://docs.signpath.io/build-system-integration>).
10. **GitHub secrets and variables** (Settings, Secrets and variables, Actions). Secret:
    `SIGNPATH_API_TOKEN`. Variables:

    | Variable | Value |
    | --- | --- |
    | `WINDOWS_SIGNING_PROVIDER` | `signpath` (`kms` or unset = the AWS KMS route) |
    | `SIGNPATH_ORGANIZATION_ID` | the organization GUID |
    | `SIGNPATH_PROJECT_SLUG` | `grimalkin` |
    | `SIGNPATH_SIGNING_POLICY_SLUG` | `release-signing` |
    | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | `binaries` |
    | `SIGNPATH_SETUP_ARTIFACT_CONFIGURATION_SLUG` | `setup` |
    | `WINDOWS_SIGNING_PUBLISHER` | exact Subject, expected `CN=SignPath Foundation, O=SignPath Foundation, L=Lewes, S=Delaware, C=US` |
    | `WINDOWS_SIGNING_ISSUER` | exact Issuer of that certificate, expected `CN=GlobalSign GCC R45 CodeSigning CA 2020, O=GlobalSign nv-sa, C=BE` |
    | `SIGNPATH_WAIT_TIMEOUT_SECONDS` | optional, default 7200 (the wait for each approval) |
    | `WINDOWS_SIGNING_ENABLED` | `true`, set last |

    ```
    gh secret set SIGNPATH_API_TOKEN
    gh variable set WINDOWS_SIGNING_PROVIDER --body signpath
    gh variable set SIGNPATH_ORGANIZATION_ID --body <guid>
    ```
11. Restrict who may create `v*` tags (a repository ruleset), exactly as for the KMS route.
12. **Release candidate:** push a `v*` tag on a trunk commit with signing enabled. Watch the
    `windows sign (signpath)` job: it waits twice; approve each request in SignPath (e-mail,
    or the signing request page the action prints). The job summary links the requests.
13. If a verify step fails on publisher or issuer, the message prints the actual value:
    correct the variable (after checking it really is SignPath's certificate) and re-run the
    failed job. A re-run submits new requests that need approval again.
14. Do the Windows acceptance checklist in the README before announcing the release.

## 8. Open questions (resolved by the first real run)

- The exact publisher and issuer strings Windows reports (section 4 gives what to expect from
  a real file; the pipeline refuses to continue if the variables differ).
- Whether SignPath accepts the artifact configuration XML as written (restrictions on
  `<pe-file-set>`, wildcard `uninst-*.exe` with the default of exactly one match) - the
  editor reports schema problems immediately. If `product-version` cannot be enforced for the
  uninstaller or Setup (their ProductVersion comes from Inno's `VersionInfoProductTextVersion`),
  ask SignPath what they enforce; `check-product-info.ps1` already checks the same values in CI.
- How "Allowed branch names" treats tag builds (leave it empty until confirmed).
- Whether SignPath's signature on the uninstaller is accepted by Inno's byte-for-byte check
  (it should be: SignPath uses standard Authenticode; the final compile fails closed if not).
- The unlisted details of the application (form fields, turnaround), quotas and file size limits.
- GitHub's repository license detection shows `NOASSERTION`; if SignPath's reviewers trip on
  it, put the SPDX expression in the license text GitHub recognises or mention the two
  licenses on the application.

## 9. Sources (all fetched 2026-10-31)

- <https://signpath.org/>, <https://signpath.org/terms>, <https://signpath.org/about>,
  <https://signpath.org/apply>, <https://signpath.org/projects>
- <https://signpath.io/solutions/open-source-community>
- <https://docs.signpath.io/trusted-build-systems/github>,
  <https://docs.signpath.io/trusted-build-systems>, <https://docs.signpath.io/origin-verification>,
  <https://docs.signpath.io/projects>, <https://docs.signpath.io/artifact-configuration>,
  <https://docs.signpath.io/artifact-configuration/syntax>,
  <https://docs.signpath.io/artifact-configuration/reference>,
  <https://docs.signpath.io/build-system-integration>, <https://docs.signpath.io/crypto-providers>,
  <https://docs.signpath.io/managing-certificates>
- <https://github.com/SignPath/github-action-submit-signing-request> (`action.yml`, tags)
- <https://jrsoftware.org/ishelp/topic_setup_signeduninstaller.htm>,
  <https://jrsoftware.org/ishelp/topic_setup_signtool.htm>,
  <https://github.com/jrsoftware/issrc/blob/main/Projects/Src/Compiler.SetupCompiler.pas>
- `bottom` 0.14.9 `btm.exe` signature (certificate subject, issuer, timestamp):
  <https://github.com/ClementTsang/bottom/releases/tag/0.14.9>
- <https://www.freefixer.com/library/publisher/SignPath%20Foundation> and
  <https://zenn.dev/shm_7ec/articles/signpath-oss-code-signing> (third-party, used only where
  stated)
