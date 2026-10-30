# AWS side of Windows release signing

Order of operations. Background, CA comparison and costs: [docs/SIGNING-AWS.md](../../docs/SIGNING-AWS.md).
The pipeline itself and the repository variables: README, "Windows release signing".

All commands assume AWS credentials of an administrator in the account that will own the key.

## 0. Prerequisites

- **CloudTrail management events must be on** in that account (reuse an existing trail;
  create one only if none exists). The CA/Browser Forum code signing rules allow a cloud
  key only with access logging, and CAs ask for evidence. KMS `Sign`, `GetPublicKey` and
  `PutKeyPolicy` calls then show up in the trail. Consider an alarm on `PutKeyPolicy`,
  `ScheduleKeyDeletion` and `Sign` outside release times.
- A GitHub **tag ruleset** (Settings, Rules) that only maintainers may create `v*` tags.
  The IAM trust policy trusts any `v*` tag of this repository, and anyone who can push
  a tag can run a workflow from it.
- The numeric owner and repository ids, which select GitHub's immutable OIDC subject:
  ```
  gh api repos/rlankenau/grimalkin/actions/oidc/customization/sub
  # {"use_immutable_subject":true,"sub_claim_prefix":"repo:rlankenau@503001/grimalkin@1437335379"}
  ```
  The defaults in the template are this repository's. For a fork, pass your own
  `GitHubOwner`, `GitHubRepo`, `GitHubOwnerId`, `GitHubRepoId` (or leave both ids empty
  if `use_immutable_subject` is false).

## 1. Deploy the stack

```
aws cloudformation deploy --stack-name grimalkin-codesign \
    --template-file infra/aws/codesign.yaml --capabilities CAPABILITY_NAMED_IAM \
    --region us-east-1
aws cloudformation update-termination-protection --enable-termination-protection \
    --stack-name grimalkin-codesign --region us-east-1
aws cloudformation describe-stacks --stack-name grimalkin-codesign --region us-east-1 \
    --query 'Stacks[0].Outputs'
```

If the account already has the GitHub OIDC provider (`aws iam list-open-id-connect-providers`),
add `--parameter-overrides ExistingOidcProviderArn=arn:aws:iam::ACCOUNT:oidc-provider/token.actions.githubusercontent.com`.

It creates the RSA_4096 `SIGN_VERIFY` key (`alias/grimalkin-codesign`, retained if the stack is
deleted), the role `grimalkin-release-signer` that only `v*` tag builds of the repository
can assume, and a key policy that lets only that role call `kms:Sign`.

## 2. Create the certificate request

The key policy lets nobody but the CI role sign, so allow your own IAM principal just for this:

```
aws cloudformation deploy --stack-name grimalkin-codesign --template-file infra/aws/codesign.yaml \
    --capabilities CAPABILITY_NAMED_IAM --region us-east-1 \
    --parameter-overrides CsrSignerPrincipalArn=$(aws sts get-caller-identity --query Arn --output text)
python3 -m venv .venv && . .venv/bin/activate && pip install boto3 cryptography
scripts/aws/make-csr.py --region us-east-1 --key alias/grimalkin-codesign \
    --common-name "Your Name or Organisation" --country US > grimalkin-codesign.csr
# remove the temporary permission again
aws cloudformation deploy --stack-name grimalkin-codesign --template-file infra/aws/codesign.yaml \
    --capabilities CAPABILITY_NAMED_IAM --region us-east-1
```

(Use the same `--parameter-overrides` as in step 1 if you changed any parameter. If your ARN is an
assumed-role session, give the role ARN instead of the session ARN.) The script signs the
request with KMS (RSASSA_PKCS1_V1_5_SHA_256); the private key is never exported. Check it:
`openssl req -in grimalkin-codesign.csr -verify -noout -text`.

## 3. Get the certificate

Send the CSR to a certification authority that accepts AWS KMS keys for code signing (see
docs/SIGNING-AWS.md for candidates, key protection evidence and prices). Which CA is your
decision. Notes:

- Get an **OV or IV** (individual validated) code signing certificate. EV no longer bypasses
  SmartScreen reputation, so it is not worth its price here.
- Public code signing certificates last at most 460 days: plan to repeat steps 2 to 4 about
  once a year, ideally a month before expiry, and keep the same subject so SmartScreen
  reputation carries over.
- You get a leaf certificate and the CA's intermediate certificate(s).

## 4. Commit the chain

Put the leaf first, then the intermediates (no root, no private key), as PEM, in
`installer/windows/codesign-chain.pem`:

```
cat leaf.pem intermediate.pem > installer/windows/codesign-chain.pem
```

Open a PR. CI does not need AWS for this. (The release job later proves that the leaf's public
key equals the KMS key's, its subject equals the variable below, and that it chains to a root
Windows trusts.)

## 5. Set the repository variables

Settings, Secrets and variables, Actions, **Variables** (none of them is secret):

| Variable | Value |
| --- | --- |
| `AWS_SIGNING_ROLE_ARN` | stack output `SignerRoleArn` |
| `AWS_REGION` | stack output `Region` |
| `AWS_KMS_KEY_ALIAS` | stack output `KeyAlias` (default `alias/grimalkin-codesign` if unset) |
| `WINDOWS_SIGNING_PUBLISHER` | the leaf's exact Subject as Windows prints it |
| `WINDOWS_SIGNING_TSA_URL` | optional, RFC 3161 TSA URL(s), comma separated; default `http://timestamp.digicert.com` |
| `WINDOWS_SIGNING_ENABLED` | set to `true` last |

The exact Subject, case-sensitive, from the committed chain (PowerShell 7):

```
. scripts/windows/signing-common.ps1
(Read-PemCertificates installer/windows/codesign-chain.pem)[0].Subject
```

The release job prints the leaf's subject and fails with both values if they differ, so a
first attempt on a throw-away tag is safe.

## 6. Tag

Tag a commit that is on trunk, `git tag v1.0.0 && git push origin v1.0.0`. With
`WINDOWS_SIGNING_ENABLED=true` the `windows sign` job assumes the role, signs, verifies,
and the assets are attached only after both architectures pass the installer smoke test.
Then do the Windows 10/11 acceptance checklist in the README.

## Renewal and rotation

- Certificate renewal: new CSR from the same KMS key (steps 2 to 4; or ask the CA to reissue
  for the same CSR), replace `codesign-chain.pem` in a PR, update `WINDOWS_SIGNING_PUBLISHER` if the
  subject changed. No workflow change. Releases signed with the old certificate stay valid
  because they carry a timestamp.
- Key rotation: deploy a second stack (or change `KeyAliasName`), new CSR and certificate,
  then repoint `AWS_KMS_KEY_ALIAS`.
- Incident (key or role suspected abused): set `WINDOWS_SIGNING_ENABLED` to `false`,
  disable the key (`aws kms disable-key`), ask the CA to revoke the certificate, review CloudTrail.
