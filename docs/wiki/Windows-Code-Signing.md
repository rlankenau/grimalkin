# Windows Code Signing

An ordered guide to getting Authenticode-signed Windows releases of grimalkin. GitHub Actions signs `v*` tags only, through one of two providers selected by the repository variable `WINDOWS_SIGNING_PROVIDER`: **`signpath`** (SignPath Foundation, free, recommended for this project) or **`kms`** (the default: a certificate you buy, with the private key in AWS KMS). Sources, all in the repo: the SignPath research and runbook [`docs/SIGNING-SIGNPATH.md`](https://github.com/rlankenau/grimalkin/blob/trunk/docs/SIGNING-SIGNPATH.md), the SignPath artifact configurations [`installer/windows/signpath/`](https://github.com/rlankenau/grimalkin/tree/trunk/installer/windows/signpath), the AWS runbook [`scripts/aws/README.md`](https://github.com/rlankenau/grimalkin/blob/trunk/scripts/aws/README.md), the research notes [`docs/SIGNING-AWS.md`](https://github.com/rlankenau/grimalkin/blob/trunk/docs/SIGNING-AWS.md), the template [`infra/aws/codesign.yaml`](https://github.com/rlankenau/grimalkin/blob/trunk/infra/aws/codesign.yaml), the script [`scripts/aws/make-csr.py`](https://github.com/rlankenau/grimalkin/blob/trunk/scripts/aws/make-csr.py), the [README section "Windows release signing"](https://github.com/rlankenau/grimalkin/blob/trunk/README.md#windows-release-signing) and [`.github/workflows/ci.yml`](https://github.com/rlankenau/grimalkin/blob/trunk/.github/workflows/ci.yml).

## Why

- Unsigned files show "Unknown publisher" and trigger SmartScreen and UAC warnings.
- A signature proves who published the file and that it was not modified. A trusted RFC 3161 timestamp keeps the signature valid after the certificate expires.
- Extended Validation (EV) no longer bypasses SmartScreen (Microsoft), so buy an OV or IV certificate, not EV.
- A valid signature alone does not give SmartScreen reputation. A new certificate may still show "Windows protected your PC" until enough clean installs accumulate (Microsoft: "several weeks and hundreds of clean installs"). Keep one signing identity across renewals so reputation carries over.

## Choosing a route

| | Route A: SignPath Foundation | Route B: AWS KMS |
| --- | --- | --- |
| Cost | free | about $12/yr AWS plus $129 to $439/yr for the certificate |
| Publisher shown by Windows | "SignPath Foundation" (not the project or you) | the certificate holder you registered |
| Key custody | SignPath's HSM; you never hold the key | non-exportable key in your AWS account |
| Renewal | none (SignPath rotates the certificate) | yearly new certificate, replace `codesign-chain.pem` |
| Release flow | two manual approvals in SignPath per release | fully automatic on a tag |
| Requirements | eligible open source project, application review, origin verification, MFA | an AWS account and a CA that accepts a KMS key |

Both routes keep `conpty.dll` and `OpenConsole.exe` out of signing, fail closed once signing is enabled, label unsigned builds `-unsigned`, and share the smoke tests and release attachment. Switching is the variable `WINDOWS_SIGNING_PROVIDER`.

# Route A: SignPath Foundation (recommended)

Facts below are from signpath.org and docs.signpath.io, read on 2026-10-31; details and sources are in [`docs/SIGNING-SIGNPATH.md`](https://github.com/rlankenau/grimalkin/blob/trunk/docs/SIGNING-SIGNPATH.md).

## A1: eligibility

Required by the [terms](https://signpath.org/terms): an OSI-approved license without commercial dual-licensing for all components (MIT OR Apache-2.0 qualifies), no proprietary components (Microsoft's ConPTY is shipped unmodified and never signed by us), actively maintained, already released in the form to be signed, functionality described on the download page, MFA for every team member on SignPath and GitHub, defined author/reviewer/approver roles, a "Code signing policy" section on the project home page (the README has it), builds from the repository in a verifiable way (GitHub Actions on GitHub-hosted runners) and manual approval of every release. No popularity or age threshold is published. The certificate is issued to SignPath Foundation, which is the publisher; the Foundation may pause the subscription or revoke the certificate for violations.

## A2: apply

Apply at <https://signpath.org/apply>. No turnaround is published (a third-party guide says a few days). After approval you receive a SignPath organization at <https://app.signpath.io>.

## A3: configure SignPath

1. Note the **Organization ID**.
2. Add the predefined trusted build system **GitHub.com** to the organization and link it to the project.
3. Create the project (slug `grimalkin`) with repository URL `https://github.com/rlankenau/grimalkin`.
4. Create two artifact configurations from the XML files in `installer/windows/signpath/`: `binaries` (program files and uninstallers) and `setup` (the two Setup executables). Both enforce product name `Grimalkin` and take the parameter `version`.
5. Signing policy `release-signing`: the Foundation's certificate, approval required (you are the approver), trusted build system verification and **Verify origin** on. Leave "Allowed branch names" empty until SignPath confirms how tag builds are matched. An optional `test-signing` policy uses an untrusted self-signed certificate, so the pipeline cannot use it.
6. Create a dedicated CI user, make it a submitter on the policy and create its API token.

## A4: GitHub secret and variables

Settings, Secrets and variables, Actions.

| Name | Kind | Value |
| --- | --- | --- |
| `SIGNPATH_API_TOKEN` | secret | the CI user's API token |
| `WINDOWS_SIGNING_PROVIDER` | variable | `signpath` |
| `SIGNPATH_ORGANIZATION_ID` | variable | the organization GUID |
| `SIGNPATH_PROJECT_SLUG` | variable | `grimalkin` |
| `SIGNPATH_SIGNING_POLICY_SLUG` | variable | `release-signing` |
| `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | variable | `binaries` |
| `SIGNPATH_SETUP_ARTIFACT_CONFIGURATION_SLUG` | variable | `setup` |
| `WINDOWS_SIGNING_PUBLISHER` | variable | exact Subject, expected `CN=SignPath Foundation, O=SignPath Foundation, L=Lewes, S=Delaware, C=US` |
| `WINDOWS_SIGNING_ISSUER` | variable | exact Issuer, expected `CN=GlobalSign GCC R45 CodeSigning CA 2020, O=GlobalSign nv-sa, C=BE` |
| `SIGNPATH_WAIT_TIMEOUT_SECONDS` | variable, optional | wait per request including approval; default 7200 |
| `WINDOWS_SIGNING_ENABLED` | variable | `true`, set last |

```
gh secret set SIGNPATH_API_TOKEN
gh variable set WINDOWS_SIGNING_PROVIDER --body signpath
gh variable set WINDOWS_SIGNING_ENABLED --body true
```

The expected publisher and issuer are what a current SignPath-signed file shows (SignPath does not document them); the verify steps print the actual values on a mismatch. The issuer can change when SignPath changes CA, which is why it is a variable and the leaf certificate (rotated by SignPath) is not pinned.

## A5: first signed release

Restrict `v*` tag creation with a repository ruleset, then push a release candidate tag on a trunk commit. In the job `windows sign (signpath)`:

1. unsigned uninstallers are generated with Inno Setup (a documented mode: `SignedUninstaller=yes` without `SignTool`; ISCC stops there by design);
2. **request 1** (`grimalkin.exe`, the launcher submitted as `grimalkin-launcher.exe` because SignPath's PE type has no `.com`, and the uninstallers, both architectures) waits for your approval in SignPath;
3. the signed files are verified (valid, embedded, exact publisher, pinned issuer, timestamp) and the Setup executables are built from them (Inno embeds the signed uninstaller and fails if it is missing);
4. **request 2** (the two Setup executables) waits for your approval;
5. verification, test twins and portable zips as in route B; then the smoke test on native x64 and ARM64 runners, which also checks the installed `unins000.exe`.

The Inno behaviour the flow relies on is tested with a self-signed stand-in on every CI run (`scripts/windows/test-signpath-inno-flow.ps1`). A failed verify step can be fixed (a wrong variable) and the job re-run; a re-run submits new requests that need approval again.

## A6: limitations

- Two approvals per release; the job waits (2 hours per request by default).
- Setup's temporary self-copies are not signed because Setup is signed after compilation (Inno signs them only through `SignTool=`, which cannot call SignPath from inside ISCC for an open source project). Check the UAC prompt of an all-users install on a real machine.
- No renewal work, but the publisher is "SignPath Foundation" and the Foundation can revoke it.

# Route B: AWS KMS and a purchased certificate

Everything below is the AWS route. Choose it when the SignPath application is declined or you want your own name as publisher. Set `WINDOWS_SIGNING_PROVIDER` to `kms` or leave it unset.

## Prerequisites

- An AWS account with administrator credentials; the commands below use the `spidergwen` CLI profile (add `--profile spidergwen`, or `export AWS_PROFILE=spidergwen`).
- CloudTrail management events on (reuse an existing trail). Code signing rules allow a cloud key only with access logging, and CAs ask for evidence. Consider alarms on `PutKeyPolicy`, `ScheduleKeyDeletion` and `Sign` outside release times.
- A GitHub tag ruleset (Settings, Rules) so only maintainers can create `v*` tags. The IAM role trusts any `v*` tag of the repo, and whoever can push a tag can run a workflow from it.
- Python 3 with `boto3` and `cryptography`, and `gh` for the OIDC lookup.
- The immutable OIDC subject ids. Repos created after 2026-07-15 use `repo:OWNER@OWNER-ID/REPO@REPO-ID:...`; a trust policy in the old format fails with "Not authorized".

```
gh api repos/rlankenau/grimalkin/actions/oidc/customization/sub
# {"use_immutable_subject":true,"sub_claim_prefix":"repo:rlankenau@503001/grimalkin@1437335379"}
```

The template defaults are these values (`GitHubOwnerId=503001`, `GitHubRepoId=1437335379`), so for this repo you pass nothing.

## Step 1: deploy the CloudFormation stack

```
export AWS_PROFILE=spidergwen
aws cloudformation deploy --stack-name grimalkin-codesign \
    --template-file infra/aws/codesign.yaml --capabilities CAPABILITY_NAMED_IAM \
    --region us-east-1
aws cloudformation update-termination-protection --enable-termination-protection \
    --stack-name grimalkin-codesign --region us-east-1
aws cloudformation describe-stacks --stack-name grimalkin-codesign --region us-east-1 \
    --query 'Stacks[0].Outputs'
```

If the account already has the GitHub OIDC provider (only one per URL is allowed), check and pass it:

```
aws iam list-open-id-connect-providers
aws cloudformation deploy --stack-name grimalkin-codesign \
    --template-file infra/aws/codesign.yaml --capabilities CAPABILITY_NAMED_IAM \
    --region us-east-1 \
    --parameter-overrides ExistingOidcProviderArn=arn:aws:iam::ACCOUNT:oidc-provider/token.actions.githubusercontent.com
```

It creates: an RSA_4096 `SIGN_VERIFY` KMS key (`alias/grimalkin-codesign`, retained if the stack is deleted), the role `grimalkin-release-signer` (assumable only by `v*` tag builds of the repo), and a key policy under which only that role can `kms:Sign` (a Deny covers everyone else, including account root). Parameters you may touch: `GitHubOwner`, `GitHubRepo`, `GitHubOwnerId`, `GitHubRepoId`, `TagPattern` (default `v*`), `ExistingOidcProviderArn`, `RoleName`, `CsrSignerPrincipalArn`, `KeyAliasName`. Note the outputs `SignerRoleArn`, `Region`, `KeyAlias`; you need them in step 5.

## Step 2: create the CSR

The key policy lets nobody but the CI role sign, so temporarily allow your own principal:

```
aws cloudformation deploy --stack-name grimalkin-codesign --template-file infra/aws/codesign.yaml \
    --capabilities CAPABILITY_NAMED_IAM --region us-east-1 \
    --parameter-overrides CsrSignerPrincipalArn=$(aws sts get-caller-identity --query Arn --output text)
```

If your ARN is an assumed-role session, pass the role ARN instead. Pass any other overrides from step 1 again if you changed them.

```
python3 -m venv .venv && . .venv/bin/activate && pip install boto3 cryptography
scripts/aws/make-csr.py --region us-east-1 --key alias/grimalkin-codesign \
    --common-name "Your Name or Organisation" --country US > grimalkin-codesign.csr
openssl req -in grimalkin-codesign.csr -verify -noout -text
```

Options: `--region` and `--key` and `--common-name` are required; `--organization`, `--locality`, `--state`, `--country` (two letters) and `--out FILE` are optional. The key is signed with RSASSA_PKCS1_V1_5_SHA_256 and never exported. Choose the subject to match what the CA will validate; the CA may override it, and whatever it issues is what goes into `WINDOWS_SIGNING_PUBLISHER`.

```
# individual (IV): your legal name
scripts/aws/make-csr.py --region us-east-1 --key alias/grimalkin-codesign \
    --common-name "Alice Lankenau" --country US --state Texas --locality Austin > grimalkin-codesign.csr
# organisation (OV): the registered entity
scripts/aws/make-csr.py --region us-east-1 --key alias/grimalkin-codesign \
    --common-name "Example LLC" --organization "Example LLC" --country US --state Texas --locality Austin > grimalkin-codesign.csr
```

Now remove the temporary permission. Pass the parameter explicitly as empty, because `deploy` keeps the previous value of any parameter you leave out:

```
aws cloudformation deploy --stack-name grimalkin-codesign --template-file infra/aws/codesign.yaml \
    --capabilities CAPABILITY_NAMED_IAM --region us-east-1 \
    --parameter-overrides "CsrSignerPrincipalArn="
aws cloudformation describe-stacks --stack-name grimalkin-codesign --region us-east-1 \
    --query 'Stacks[0].Parameters[?ParameterKey==`CsrSignerPrincipalArn`]'
```

The value must be empty or absent.

## Step 3: obtain the certificate

Which CA is your decision. Get an OV or IV certificate. Ranked options from [`docs/SIGNING-AWS.md`](https://github.com/rlankenau/grimalkin/blob/trunk/docs/SIGNING-AWS.md) (research dated 2026-10-30; prices change, confirm with the CA):

1. **GlobalSign OV, HSM-based, CSR from KMS.** The only CA with an official AWS KMS walkthrough (dated 2026-10-14). Price not published: get a quote. Unknowns: whether a sole proprietor qualifies for OV, and whether it still asks for a key-protection report.
2. **SSL.com IV or OV, $129/yr each**, via the BYOA / attestation-letter path. Cheapest and individual-friendly (IV needs only government ID). KMS is not on their list; ask first whether a CloudTrail/KMS configuration report or an auditor-witnessed KMS key creation is accepted, and the auditor fee (unpublished). Their CloudHSM ceremony costs $1,200-1,275, which would erase the saving.
3. **DigiCert OV, about $439/yr via resellers**, "HSM (CSR required)" method: attestation by agreement email. KMS acceptance is unconfirmed.
4. **Entrust.** Its knowledge-base article names AWS KMS, but the page could not be fetched and availability and price are unknown.
5. Sectigo and Certum: not viable for KMS on current evidence. Do not buy EV.

If no CA accepts a KMS key, the notes list non-AWS fallbacks (SSL.com eSigner, Microsoft Artifact Signing at $9.99/month), which would need a different pipeline than this one.

What to tell the CA:

- The key is generated and held in AWS KMS (RSA_4096, `SIGN_VERIFY`), never exported; KMS runs on HSMs validated at FIPS 140-3 Security Level 3 per AWS.
- This is cloud key protection under the CA/Browser Forum Code Signing Baseline Requirements section 6.2.7.4.1 (8); all access is logged with CloudTrail.
- AWS KMS has no remote key-attestation document, so ask for verification by a configuration report or witnessed creation (CSBR 6.2.7.4.2 methods 5 or 6). Useful evidence: the CSR, the CloudTrail `CreateKey` event, and the CloudTrail trail configuration.

Validation evidence: IV (individual) needs a government photo ID and a phone callback, with no business documents. OV needs a business entity and registration documents. Expect a validity of at most 460 days (certificates issued on or after 2026-03-01); GlobalSign now sells 1-year (366 day) certificates only. You receive a leaf certificate plus the CA's intermediate(s).

## Step 4: commit the chain

Put the leaf first, then the intermediates, as PEM, in [`installer/windows/codesign-chain.pem`](https://github.com/rlankenau/grimalkin/blob/trunk/installer/windows/codesign-chain.pem). No root, no private key (public certificates are not secrets).

```
cat leaf.pem intermediate.pem > installer/windows/codesign-chain.pem
```

Print the exact, case-sensitive Subject as Windows shows it (PowerShell 7) for `WINDOWS_SIGNING_PUBLISHER`:

```
. scripts/windows/signing-common.ps1
(Read-PemCertificates installer/windows/codesign-chain.pem)[0].Subject
```

Open a PR and merge to trunk. CI needs no AWS for this. The file is a placeholder until now; the gate fails a signing run if it holds no certificate.

## Step 5: set repository variables

Settings, Secrets and variables, Actions, Variables. None is secret.

| Name | Example value | Required? |
| --- | --- | --- |
| `AWS_SIGNING_ROLE_ARN` | `arn:aws:iam::123456789012:role/grimalkin-release-signer` (stack output `SignerRoleArn`) | Yes |
| `AWS_REGION` | `us-east-1` (stack output `Region`) | Yes |
| `WINDOWS_SIGNING_PUBLISHER` | the exact leaf Subject from step 4 | Yes |
| `AWS_KMS_KEY_ALIAS` | `alias/grimalkin-codesign` (stack output `KeyAlias`) | No, that is the default |
| `WINDOWS_SIGNING_TSA_URL` | `http://timestamp.digicert.com,http://timestamp.sectigo.com` | No; comma-separated fallbacks; default `http://timestamp.digicert.com` |
| `WINDOWS_SIGNING_ENABLED` | `true` | Yes, set it last |
| `WINDOWS_SIGNING_PROVIDER` | `kms` | No, that is the default |

```
gh variable set AWS_SIGNING_ROLE_ARN --body "arn:aws:iam::ACCOUNT:role/grimalkin-release-signer"
gh variable set AWS_REGION --body us-east-1
gh variable set WINDOWS_SIGNING_PUBLISHER --body "<exact Subject>"
gh variable set WINDOWS_SIGNING_ENABLED --body true
```

## Step 6: first signed release

Use a throwaway tag on a commit that is on trunk (the gate refuses tags that are not ancestors of `trunk`). The publisher check prints both values on a mismatch, so a first attempt is safe.

```
git tag v1.0.0-signtest && git push origin v1.0.0-signtest
```

Jobs, in order: `windows gate`, `windows build` (x64, arm64), `windows sign`, `windows smoke` (x64, arm64), then `windows release assets`.

- **windows gate** (provider `kms`, the default) signs only if `WINDOWS_SIGNING_ENABLED` is `true`, the event is a push of a `v*` tag on `rlankenau/grimalkin`, and the commit is on trunk. Once signing is enabled on a release tag, a missing variable (`AWS_SIGNING_ROLE_ARN`, `AWS_REGION`, `WINDOWS_SIGNING_PUBLISHER`), a role ARN that is not an IAM role, the placeholder chain, or a tag off trunk fails the run. There is no silent unsigned fallback and nothing is uploaded.
- **windows sign** verifies the pinned jsign jar by SHA-256, assumes the role via OIDC, proves the committed certificate belongs to the KMS key and that its subject equals the publisher variable, signs `grimalkin.exe` and `grimalkin.com` for both architectures and verifies each, runs negative tests, builds the installers with Inno Setup (Setup and uninstaller signed), builds the zips, and uploads.
- **windows smoke** installs, checks and uninstalls on native x64 and ARM64 runners, and for signed builds verifies Setup, the installed files and the uninstaller.
- Release assets are attached only after both smoke jobs pass.

When it fails, open the failing job in the Actions tab: the gate prints `::error::` lines, and `windows sign` has one step per check (assume role, chain check, sign and verify). The run summary shows `windows signing: true/false`.

When the gate is off (variable not `true`, not a `v*` tag, or a fork), the `windows sign` job is skipped, nothing is signed, and the installer and zip are named `grimalkin-setup-<arch>-unsigned.exe` and `grimalkin-portable-<arch>-unsigned.zip`, with `UNSIGNED.txt` in the zip. Signed releases have the plain names. Delete the throwaway tag and release afterwards.

## Step 7: verify on a Windows machine

Download the installer through a browser (so Mark of the Web is set), then:

```
signtool verify /pa /all /tw /v grimalkin-setup-x64.exe
```

Exit 0 is OK, 1 fail, 2 warning; treat 2 as a failure. Also check Properties, Digital Signatures: signer is the expected publisher, a counter-signature (timestamp) is present and "This digital signature is OK". Repeat on installed `grimalkin.exe`, `grimalkin.com` and `unins000.exe`, and test x64 Windows 10, x64 Windows 11 and Windows 11 ARM64 per the README's acceptance checklist. `conpty.dll` and `OpenConsole.exe` should show Microsoft Corporation (they are never re-signed).

Expect the UAC dialog to show the verified publisher. A new certificate may still produce a SmartScreen "unrecognized" warning at first; that is reputation, not a failed signature, and it fades as installs accumulate.

## Renewal and rotation

- **Yearly renewal.** Do it about a month before expiry. Make a new CSR from the same KMS key (repeat steps 2-4, or ask the CA to reissue for the same CSR), replace `codesign-chain.pem` in a PR, and update `WINDOWS_SIGNING_PUBLISHER` if the subject changed. Keep the subject the same so reputation carries over. No workflow change.
- **Key rotation.** Changing `KeyAliasName` does not rotate anything; it re-points an alias at the same key. A new key needs a second stack:

```
aws cloudformation deploy --stack-name grimalkin-codesign-2 \
    --template-file infra/aws/codesign.yaml --capabilities CAPABILITY_NAMED_IAM \
    --region us-east-1 \
    --parameter-overrides RoleName=grimalkin-release-signer-2 KeyAliasName=alias/grimalkin-codesign-2 \
    ExistingOidcProviderArn=arn:aws:iam::ACCOUNT:oidc-provider/token.actions.githubusercontent.com \
    CsrSignerPrincipalArn=<your ARN>
```

  Then CSR, certificate, chain and publisher as in steps 2-4, point `AWS_KMS_KEY_ALIAS` and `AWS_SIGNING_ROLE_ARN` at the new stack, cut a release, clear `CsrSignerPrincipalArn`, and schedule the old key for deletion (the template retains the key, so deleting a stack does not delete it):

```
aws kms schedule-key-deletion --key-id <old-key-id> --pending-window-in-days 30
```

- Signatures that carry a timestamp stay valid after the key and its certificate are gone.
- **Incident** (key or role suspected abused): set `WINDOWS_SIGNING_ENABLED` to `false`, run `aws kms disable-key --key-id <key-id>`, ask the CA to revoke the certificate, review CloudTrail.

## Troubleshooting

- **"Not authorized" assuming the role.** The OIDC subject format does not match. Re-read the prefix with the `gh api ... customization/sub` command, compare with the stack output `OidcSubjectTrusted`, and confirm the job runs on a `v*` tag (an environment-based job has a different `sub`). A fork's ids differ from the defaults.
- **KMS AccessDenied.** The key policy allows only the role (plus `CsrSignerPrincipalArn` while set) to `Sign`, with an explicit Deny for everyone else. Check the role ARN and signing algorithm (RSASSA_PKCS1_V1_5 SHA-256/384/512 only). jsign also calls `kms:ListKeys`, granted by the role's identity policy. The research notes say `ListKeys` may not be strictly needed but this is untested.
- **Chain/leaf mismatch.** The sign job fails if the leaf's public key is not the KMS key's. Check the chain starts with the leaf and the certificate was issued for the CSR made from this key. Compare `aws kms get-public-key` with `openssl x509 -pubkey`.
- **Publisher string mismatch.** The variable must equal the leaf Subject exactly, case-sensitive. Regenerate it with the PowerShell one-liner in step 4; the job prints both values.
- **Placeholder chain.** `installer/windows/codesign-chain.pem` must contain a certificate and be merged to trunk before tagging.
- **TSA timeouts.** jsign retries 3 times with a 10 s wait, then the whole signing call retries. Supply fallbacks in `WINDOWS_SIGNING_TSA_URL` (DigiCert first, Sectigo second); Sectigo asks for 15 s or more between scripted requests. Timestamps must be RFC 3161 (jsign's default mode is not).
- **Inno Setup SignTool quoting.** Setup and the uninstaller are signed through `SignTool=` which calls `scripts/windows/sign-file.ps1`; `$f` is the quoted filename. If signing the installer fails but the exe step worked, look at that step's log first.
- **Tag not signed.** The tag is off trunk, the repo is a fork, or `WINDOWS_SIGNING_ENABLED` is not exactly `true`.
- **Your principal can still sign after step 2.** You left out the explicit empty `CsrSignerPrincipalArn=` override; redeploy with it and verify with `describe-stacks`.

## Cost summary

- AWS: KMS key $1/month (about $12/yr), signing requests under $0.01 per release (a release signs roughly 12-24 times). CloudTrail's first management trail is free; the IAM OIDC provider and roles are free. About $12-13/yr.
- Certificate: SSL.com IV or OV $129/yr; DigiCert OV about $439/yr via resellers; GlobalSign by quote. SSL.com may add an attestation fee (unpublished).
- First-year estimates from the notes: about $129 + $12 + unknown attestation (SSL.com), about $451 (DigiCert).
