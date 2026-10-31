# Authenticode signing with an AWS KMS key (research notes)

Research date: 2026-10-30. Scope: sign `grimalkin.exe`, `grimalkin.com`, the Inno Setup
installer and its uninstaller, for x64 and arm64, from GitHub Actions, with a private key that
is generated in and never leaves AWS KMS, no long-lived secrets, a publicly trusted
certificate and RFC 3161 timestamps. "Verified" = read on the cited page or in cited source on
this date. "Not found" = searched, no official statement located.

## 1. CA/B Forum rules and which CAs accept a KMS key

**Rule.** Code Signing Baseline Requirements (CSBR) v3.11.0 (2026-06-16), source:
<https://github.com/cabforum/code-signing/blob/main/docs/CSBR.md> (rendered:
<https://cabforum.org/working-groups/code-signing/requirements/>)
- Since 2023-06-01 (s6.2.7.4.1) the CA must obtain a contractual representation that the key is
  generated and protected in a hardware module certified to FIPS 140-2 Level 2 or Common
  Criteria EAL 4+. Allowed: (7) own hardware module, (8) "cloud-base key generation and
  protection solution" where key creation, storage and use stay inside the cloud HSM boundary
  **and** the subscription logs all access, operations and configuration changes (so enable
  CloudTrail), (9) a signing service.
- s6.2.7.4.2: the CA must verify it using one of: (2) a manufacturer-certificate "key
  attestation" counter-signed CSR; (4) an IT audit; (5) "a suitable report from the cloud-based
  key protection solution subscription and resources configuration"; (6) a report by a
  CA-approved auditor who witnesses key-pair creation, including in a cloud solution.
- s6.1.5: RSA >= 3072 (since 2021-06-01). s6.3.2: certificates issued on/after 2026-03-01 may
  not exceed 460 days. So expect to renew (new cert, same or new key) about every year.

**AWS side.** KMS key generation and asymmetric operations happen inside HSMs, the private key
never leaves; the module is "validated at FIPS 140-3 Security Level 3"
(<https://aws.amazon.com/kms/features/>). AWS KMS has no remote key-attestation document, so
CA-side acceptance is by CSBR methods 5/6 (report or witnessed ceremony), not method 2.
Key spec is fixed at creation (<https://docs.aws.amazon.com/kms/latest/developerguide/asymmetric-key-specs.html>);
RSA_2048/3072/4096 exist for SIGN_VERIFY; use **RSA_4096** (GlobalSign wants >= 4096 for HSM
CSRs, DigiCert and SSL.com >= 3072).

| CA | Evidence it takes an AWS KMS key | Source |
|---|---|---|
| GlobalSign | Best fit. Official guide (dated 2026-10-14): create RSA_4096 SIGN_VERIFY KMS key, re-sign a CSR with it, order "Code Signing Certificate (HSM-based)" with CSR key option, vetting, install; no attestation step described. Product page also lists AWS CloudHSM. Only 1-year (366 day) certs now; 2/3-year ended 2025-12-26. Prices not shown on page. | <https://support.globalsign.com/code-signing/code-signing/automate-code-signing-jenkins-aws-kms-and-signtool>, <https://www.globalsign.com/en/code-signing-certificate> |
| Entrust | KB "Code Signing Private Key Protection Requirements for Cloud HSM Providers" lists Amazon KMS and CloudHSM; needs key origin AWS KMS, asymmetric, SIGN_VERIFY, RSA 3072/4096 and a CloudTrail trail. Page returned 403 to fetch; content seen only via search excerpts. Re-check availability and price directly. | <https://www.entrust.com/knowledgebase/ssl/code-signing-private-key-protection-requirements-for-cloud-hsm-providers> |
| SSL.com | KMS not named anywhere. Product pages offer "your own cloud HSM" with attestation fees: AWS CloudHSM $1,275, Google/Azure $75, or "attestation letter you provide: no SSL.com fee". Their BYOA process (witnessed key-generation ceremony by an SSL.com-approved qualified auditor) exists because "AWS does not provide remote key attestation"; CloudHSM ceremony fee $1,200. Auditor fee not published. Must ask support if a KMS ceremony is accepted. | <https://www.ssl.com/products/software-integrity/code-signing/iv/>, <https://ssl.com/guide/supported-cloud-hsms-document-signing-ev-code-signing/>, <https://ssl.com/guide/what-is-byoa/> |
| DigiCert | "HSM (CSR required)" method: RSA >= 3072, FIPS 140-2 L2/EAL4+ "or equivalent", you confirm at order, DigiCert emails an agreement, no cryptographic attestation. Cloud KMS not mentioned in docs (only CloudHSM via partner guides). An AWS re:Post poster reports DigiCert did not require remote attestation (undated, anecdotal). | <https://docs.digicert.com/en/certcentral/order-and-manage-certificates/request-certificates/request-a-code-signing-or-ev-code-signing-certificate/code-signing-provisioning-methods.html>, <https://www.repost.aws/questions/QUGCqBSi6XTdWrgLhHTxQLkg/cloudhsm-linking-with-digicert-authenticode-certs-after-june-2023-requirement-for-hsms> |
| Sectigo | Enrollment form requires "the attestation value, and the HSM type"; template must have Key Attestation enabled. Sectigo's own guidance names YubiKey; reseller/Sectigo-store guide covers Google Cloud KMS only. No AWS statement found: unlikely to work. | <https://www.sectigo.com/knowledge-base/detail/key-attestation-for-code-signing> (updated 2026-08-14) |
| Certum | No AWS KMS documentation found. | n/a |

**Validation types / evidence (organization vs individual).**
- SSL.com IV: personal name on the cert, no business documents, government photo ID, phone
  callback; keys must be in hardware or cloud signing. IV and OV are both $129/yr (1 year;
  multi-year listed at $96.75-$116.10/yr effective, but 460-day cap means annual reissue).
  EV Sole Proprietor (individual, no registered entity) $359/yr.
  <https://www.ssl.com/products/software-integrity/code-signing/> (+ /iv/, /ov/, /ev-sole-proprietor/),
  <https://www.ssl.com/how-to/how-to-upload-validation-documents-for-ssl-com-certificates/>
- OV needs a business entity and registration documents (SSL.com OV page above). GlobalSign's
  page says to select the region "where your Company is legally registered"; whether it accepts
  an unincorporated individual was not found.
- Reseller price survey (not CA price lists; Oct 2026): Sectigo OV ~$220-322/yr, EV ~$290-500;
  DigiCert OV ~$439, EV ~$620-644. <https://sslinsights.com/digicert-vs-sectigo-code-signing/>

**EV vs OV and SmartScreen.** Microsoft (page updated 2026-08-17): "EV certificates no longer
bypass SmartScreen... Paying a premium for EV solely to avoid SmartScreen warnings is no longer
justified." A new OV or EV cert both show an "unrecognized" warning at first; reputation builds
organically over "several weeks and hundreds of clean installs"; keep one signing identity.
<https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation>
Conclusion: buy OV or IV, not EV.

## 2. jsign mechanics (verified against jsign 7.6 source and docs)

- Docs: <https://ebourg.github.io/jsign/>. AWS support added in **5.0** (2023-06-06;
  <https://github.com/ebourg/jsign/releases/tag/5.0>). Current release 7.6 (2026-10-12).
  Jar: `jsign-7.6.jar`, sha256 `6cf58aa080e7ff82656f320cff8d4a3bff30408df8766806dcba3740dd839fa5`
  (matches the GitHub release digest). Needs Java (docs example uses Temurin 17).
- Options: `--storetype AWS`, `--keystore <region>` (e.g. `us-east-1`), `--alias <key-id | alias
  | arn>` (`KeyStoreType.AWS`, `AmazonSigningService.normalizeKeyId`: a bare UUID or `arn:`
  is used as a key id, anything else gets `alias/` prefixed), `--certfile <chain>` (required;
  KMS holds no certificate; docs: PKCS#7 `.p7b`/`.spc` or PEM, example uses `full-chain.pem`),
  `--tsaurl url1,url2` (comma list = fallbacks), `--tsretries N` (default 3), `--tsretrywait`
  (default 10 s), `--alg SHA-256`.
- **Timestamp mode trap:** `--tsmode` defaults to **Authenticode**, and for PE files jsign uses
  that default (`AuthenticodeSigner.createSignedData`). Pass `--tsmode RFC3161` explicitly.
- Key support: `DescribeKey` must show `KeyUsage=SIGN_VERIFY` and `Enabled`; algorithm family
  comes from the key spec (RSA/ECC); RSA signs with RSASSA_PKCS1_V1_5_SHA_256/384/512 (and PSS).
  RSA_3072 and RSA_4096 both work (no size check in source).
- IAM actions used: docs say `kms:ListKeys`, `kms:DescribeKey`, `kms:Sign`. In the source
  `ListKeys` is only called when enumerating aliases (`aliases()`), not when `--alias` is given,
  so `Sign` + `DescribeKey` should suffice (add `GetPublicKey` for your own checks). Test once.
- **Credentials:** jsign does **not** use the AWS SDK default chain. `AmazonCredentials.getDefault()`
  reads env `AWS_ACCESS_KEY_ID` (or `AWS_ACCESS_KEY`), `AWS_SECRET_ACCESS_KEY` (or
  `AWS_SECRET_KEY`), `AWS_SESSION_TOKEN`, else ECS credentials endpoint, else EC2 IMDSv2. It has
  no web-identity-token-file, profile or SSO support. This is fine in CI because
  `aws-actions/configure-aws-credentials` performs the OIDC exchange and by default exports those
  three variables (action README). Do not pass `--storepass`. `AWS_USE_FIPS_ENDPOINT=true`
  forces FIPS endpoints.
- ARM64: docs never mention it; `PEFile.java` has no machine-type checks (grep), so it signs
  ARM64 PE files like x64. Confirm with `signtool verify` on `windows-11-arm`.
- Inno Setup: `SignTool=` signs Setup, the uninstaller and temp self-copies; the repo's
  `grimalkin.iss` already sets `SignedUninstaller=yes`, `SignToolRetryCount=3` under
  `/DSignInstaller`; define the tool on the command line, e.g.
  `ISCC /Sgrimalkin="java -jar jsign-7.6.jar --storetype AWS --keystore us-east-1 --alias KEYID --certfile chain.pem --tsmode RFC3161 --tsaurl http://timestamp.digicert.com --alg SHA-256 \$f"`
  (<https://jrsoftware.org/ishelp/topic_setup_signtool.htm>; `$f` is the quoted file name).
- Example (jsign docs shape): `jsign --storetype AWS --keystore us-east-1 --alias <key-uuid>
  --certfile chain.pem --tsmode RFC3161 --tsaurl http://timestamp.digicert.com,http://timestamp.sectigo.com
  --tsretries 5 --alg SHA-256 grimalkin.exe grimalkin.com`

**CSR for a KMS key (no jsign command).** Simplest documented route: `aws-kms-sign-csr`
(<https://github.com/g-a-d/aws-kms-sign-csr>, MIT, last code commit 2023-01-21 `4d426e2`):
```
openssl req -new -newkey rsa:4096 -nodes -keyout /dev/null -out template.csr -subj "/CN=.../O=..."
python3 -m venv v && . v/bin/activate && pip install boto3 pyasn1 pyasn1-modules
python3 aws-kms-sign-csr.py --region us-east-1 --keyid <KEY-ID> --hashalgo sha256 template.csr > kms.csr
```
It swaps in the KMS public key (`GetPublicKey`) and signs `certificationRequestInfo` with
`Sign` (RSASSA_PKCS1_V1_5_SHA_256); RSA and ECDSA only. Its `requirements.txt` pins very old
boto3 1.10.34, so install current versions. GlobalSign's guide uses the same script. Check
`openssl req -in kms.csr -verify -noout -text` shows 4096-bit RSA. Alternative: the
`aws-kms-pkcs11` module (<https://github.com/JackOfMostTrades/aws-kms-pkcs11>, MIT, v0.0.19
2026-10-21) lets `openssl req -new -key pkcs11:token=<slot> -keyform engine -engine pkcs11`
create the CSR directly (its README shows this against a KMS key).

**osslsigncode fallback.** Supports PKCS#11 engine/provider (`-pkcs11engine/-pkcs11module/-key
pkcs11:...`), EXE/MSI/CAB (<https://github.com/mtrojnar/osslsigncode>, 2.14 2026-07-20). The
aws-kms-pkcs11 README gives an osslsigncode command for KMS. It is Linux-centric and needs a
native module build; jsign (pure Java, runs on the Windows runners) is the simpler path.

## 3. GitHub Actions OIDC to AWS

1. IAM OIDC provider: URL `https://token.actions.githubusercontent.com`, audience
   `sts.amazonaws.com`. A thumbprint is only a fallback; AWS verifies the JWKS TLS cert with its
   own trusted CA library (<https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_providers_create_oidc.html>).
2. **Subject claim format has changed.** Repos created after 2026-07-15 get immutable `sub`
   values `repo:OWNER@OWNER-ID/REPO@REPO-ID:...`
   (<https://github.blog/changelog/2026-04-23-immutable-subject-claims-for-github-actions-oidc-tokens>,
   <https://docs.github.com/en/actions/reference/security/oidc>). This repo was created
   2026-10-28 and `gh api repos/rlankenau/grimalkin/actions/oidc/customization/sub` returned
   `{"use_immutable_subject":true,"sub_claim_prefix":"repo:rlankenau@503001/grimalkin@1437335379"}`.
   A trust policy written as `repo:rlankenau/grimalkin:...` will fail with "Not authorized"
   (configure-aws-credentials README).
3. Tag-triggered job `sub` is `<prefix>:ref:refs/tags/<tag>`. If the job references a GitHub
   environment the `sub` is `<prefix>:environment:<name>` instead of the tag form (GitHub OIDC
   docs, default-format examples; the immutable environment form is not documented, check with
   the sub API above). Trust policy for tag-only:
```json
{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
 "Principal":{"Federated":"arn:aws:iam::<ACCT>:oidc-provider/token.actions.githubusercontent.com"},
 "Action":"sts:AssumeRoleWithWebIdentity",
 "Condition":{"StringEquals":{"token.actions.githubusercontent.com:aud":"sts.amazonaws.com"},
  "StringLike":{"token.actions.githubusercontent.com:sub":"repo:rlankenau@503001/grimalkin@1437335379:ref:refs/tags/v*"}}}]}
```
   Note: a `v*` tag can be pushed by anyone with write access; add a tag protection ruleset,
   or use an environment with required reviewers and trust the `environment:` sub instead.
4. Workflow step (job needs `permissions: id-token: write`):
   `aws-actions/configure-aws-credentials@e1253824e5c10ff9df46874f81ed3ec929e19cfd # v6.3.0`
   (v6.3.0, 2026-09-15; SHA resolved via GitHub API) with `role-to-assume:
   arn:aws:iam::<ACCT>:role/grimalkin-release-signer`, `aws-region: us-east-1`. Also pin
   `actions/setup-java` (latest tag v6.1.0) by SHA and verify the jsign jar's sha256 in CI.
5. Role permissions (identity policy), resource = the one key:
   `kms:Sign`, `kms:GetPublicKey`, `kms:DescribeKey` on `arn:aws:kms:us-east-1:<ACCT>:key/<KEY-ID>`.
   jsign's docs also list `kms:ListKeys`; IAM cannot scope it to a key (believed `Resource: "*"`,
   not verified: the service-authorization page did not render), so add it as a separate statement
   only if a test run without it fails. Read the owner/repo ids with the sub API above.
6. KMS key policy (keeps account-root for break-glass; denies Sign to everyone else; no one,
   including the admin role, can Sign except the CI role). Key-policy semantics:
   <https://docs.aws.amazon.com/kms/latest/developerguide/key-policy-overview.html>
```json
{"Version":"2012-10-17","Statement":[
 {"Sid":"RootBreakGlass","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<ACCT>:root"},"Action":"kms:*","Resource":"*"},
 {"Sid":"SignerSign","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<ACCT>:role/grimalkin-release-signer"},
  "Action":"kms:Sign","Resource":"*",
  "Condition":{"StringEquals":{"kms:SigningAlgorithm":["RSASSA_PKCS1_V1_5_SHA_256","RSASSA_PKCS1_V1_5_SHA_384","RSASSA_PKCS1_V1_5_SHA_512"]}}},
 {"Sid":"SignerRead","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<ACCT>:role/grimalkin-release-signer"},
  "Action":["kms:GetPublicKey","kms:DescribeKey"],"Resource":"*"},
 {"Sid":"DenySignToAllElse","Effect":"Deny","Principal":{"AWS":"*"},"Action":"kms:Sign","Resource":"*",
  "Condition":{"StringNotEquals":{"aws:PrincipalArn":"arn:aws:iam::<ACCT>:role/grimalkin-release-signer"}}}]}
```
   Anyone who can `PutKeyPolicy` can rewrite this, so keep that to one human admin role and
   alert on `PutKeyPolicy`/`ScheduleKeyDeletion` in CloudTrail.
   CloudTrail logs every KMS call including Sign
   (<https://docs.aws.amazon.com/kms/latest/developerguide/logging-using-cloudtrail.html>);
   the first management-event trail copy is free (<https://aws.amazon.com/cloudtrail/pricing/>);
   a trail also satisfies CSBR s6.2.7.4.1 (8)(2) and Entrust's prerequisite.

## 4. Timestamps and verification

| TSA | RFC 3161 URL | Source |
|---|---|---|
| DigiCert | `http://timestamp.digicert.com` (lowercase; KB modified 2026-09-19) | <https://knowledge.digicert.com/general-information/rfc3161-compliant-time-stamp-authority-server> |
| Sectigo | `http://timestamp.sectigo.com` (supports RFC 3161 and Authenticode; asks >= 15 s between scripted requests) | <https://sectigo.com/resource-library/time-stamping-server> |
| GlobalSign | `http://timestamp.globalsign.com/tsa/r45standard` (old `.../r6advanced1` redirected from 2026-03-04) | <https://tbs-certificates.co.uk/FAQ/en/serveur-horodatage-globalsign-2025.html> (reseller notice relaying GlobalSign) |
| SSL.com | `http://ts.ssl.com` (community-listed, not confirmed on an ssl.com page) | <https://gist.github.com/Manouchehri/fd754e402d98430243455713efada710> |

Use DigiCert first, Sectigo second as jsign fallbacks. Timestamps keep signatures valid after the
460-day certificate expires. Verify every signed file on Windows runners (x64 and arm64):
`signtool verify /pa /all /tw /v file` (`/pa` default authentication policy, `/all` all
signatures, `/tw` warn if not timestamped, `/v` verbose; exit 0 ok, 1 fail, 2 warning)
(<https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool>). Treat exit 2 as failure.
Also check that the certificate public key equals the KMS key
(`aws kms get-public-key` vs `openssl x509 -pubkey`), and run signtool against the installer
and the installed uninstaller (`unins000.exe`).

## 5. Recommendation

Ranked options (sole developer, US, KMS key):
1. **GlobalSign OV, HSM-based, CSR from KMS.** Only CA with an official AWS KMS walkthrough
   (2026-10-14). Risks: price not published (get a quote; reseller data is stale), organization
   validation (ask if a sole proprietor with a DBA/state registration qualifies), may still ask
   for a CSBR method 5/6 report. Cost = quote + about $12/yr AWS.
2. **SSL.com IV ($129/yr) or OV ($129/yr), BYOA/attestation-letter path.** Cheapest and the
   individual-friendly one (government ID only). KMS is not on their list, so email SSL.com
   first and ask whether a CloudTrail/KMS configuration report or an auditor-witnessed KMS key
   creation is accepted and what the auditor costs; their CloudHSM ceremony is $1,200-1,275,
   which would erase the saving. First-year estimate: $129 + $12 AWS + unknown attestation fee.
3. **DigiCert OV (about $439/yr via resellers) with "HSM (CSR required)".** Documented
   own-HSM path, attestation by agreement email; KMS acceptance unconfirmed. About $451 first
   year including AWS.
4. **Entrust.** KB explicitly names AWS KMS but page unfetchable and availability/price unknown.
5. Sectigo and Certum: not viable for KMS on current evidence. Do not buy EV (SmartScreen).
Non-AWS fallbacks if no CA accepts KMS: SSL.com eSigner (they hold the key) or Microsoft Artifact
Signing, $9.99/month, no hardware token (<https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation>).

Ordered steps:
1. Make sure CloudTrail management events are on (reuse an existing trail; create one only if none).
2. Create the key: `aws kms create-key --key-spec RSA_4096 --key-usage SIGN_VERIFY --origin
   AWS_KMS --policy file://key-policy.json --description "grimalkin code signing"`; record the
   key UUID. Spec and usage cannot be changed later.
3. Generate the CSR with `aws-kms-sign-csr` (section 2); keep `kms.csr` and the CloudTrail
   `CreateKey` event as key-creation evidence for the CA.
4. Email the shortlisted CA (items in section 1) before paying; then order, complete
   identity/org vetting and key-protection verification.
5. Download the certificate and intermediates; build `chain.pem` (leaf + intermediates; do not
   include the root); commit only this public file (for example `windows/signing/chain.pem`).
   Public certs are not secrets. Confirm the leaf public key equals the KMS key.
6. Create the IAM OIDC provider, the `grimalkin-release-signer` role (trust policy section 3,
   identity policy on one key), and attach the final key policy.
7. CI: signing job only on `v*` tags with `permissions: id-token: write`; configure-aws-credentials
   (pinned SHA) then run jsign on `dist/grimalkin.exe` and `dist/grimalkin.com` before
   packaging, and ISCC with `/DSignInstaller` and the `/S` tool for Setup and uninstaller; run
   `signtool verify /pa /all /tw /v` on all outputs for both architectures. Calendar a
   renewal ~30 days before the 460-day limit (CSBR s6.3.2); same signing identity avoids
   resetting SmartScreen publisher reputation.

**AWS cost.** KMS key $1/month ($12/yr), prorated hourly, same price for asymmetric keys
(<https://aws.amazon.com/kms/pricing/>). The free tier of 20,000 requests excludes asymmetric
operations (Sign, GetPublicKey). The per-10k rate for RSA signing was not extractable from the
current page (an AWS 2022 slide lists $0.15 per 10,000; other sources differ). A release signs
roughly 3-6 files per architecture (exe, com, Setup, uninstaller, temp copies), so about 12-24
`Sign` calls: well under $0.01 per release at any of those rates. CloudTrail first trail copy
is free; S3 log storage is cents. IAM OIDC provider and roles are free. Total about
$12-13/yr, plus the certificate.
