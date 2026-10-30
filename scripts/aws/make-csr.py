#!/usr/bin/env python3
"""Build a PKCS#10 certificate signing request for an AWS KMS signing key.

The private key never leaves KMS: the public key comes from kms:GetPublicKey and the
CSR is signed by kms:Sign (RSASSA_PKCS1_V1_5_SHA_256). Send the printed PEM to your
certificate authority.

Requires python3, boto3 and cryptography (pip install boto3 cryptography) and AWS
credentials that may call kms:GetPublicKey and kms:Sign on the key. The key policy
lets only the CI role sign, so deploy (or update) the stack once with the parameter
CsrSignerPrincipalArn set to your IAM user/role ARN, run this script, then update the
stack again with that parameter empty (scripts/aws/README.md, step 2).

Example:
  scripts/aws/make-csr.py --region us-east-1 --key alias/grimalkin-codesign \\
      --common-name "Alice Lankenau" --country US --state Texas --locality Austin \\
      > grimalkin-codesign.csr

The subject you put here must be what the CA validates and will issue; the CA
may override it. Whatever it issues goes into WINDOWS_SIGNING_PUBLISHER verbatim.
"""
import argparse
import sys

import boto3
from cryptography import x509
from cryptography.hazmat.primitives import serialization
from cryptography.x509.oid import NameOID

# DER building blocks. A CSR is small and fixed in shape, so encode it by hand and
# let the KMS key sign the bytes; `cryptography` cannot sign with an external key.
SHA256_WITH_RSA = bytes.fromhex("300d06092a864886f70d01010b0500")  # AlgorithmIdentifier


def der(tag: int, body: bytes) -> bytes:
    n = len(body)
    if n < 0x80:
        length = bytes([n])
    else:
        raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
        length = bytes([0x80 | len(raw)]) + raw
    return bytes([tag]) + length + body


def build_csr(spki_der: bytes, subject: x509.Name, sign) -> x509.CertificateSigningRequest:
    """`sign(tbs: bytes) -> signature` must be RSASSA-PKCS1-v1_5 with SHA-256."""
    info = der(0x30, der(0x02, b"\x00") + subject.public_bytes() + spki_der + der(0xA0, b""))
    signature = sign(info)
    csr_der = der(0x30, info + SHA256_WITH_RSA + der(0x03, b"\x00" + signature))
    csr = x509.load_der_x509_csr(csr_der)
    if not csr.is_signature_valid:
        raise SystemExit("internal error: the CSR signature does not verify")
    return csr


def make_subject(args) -> x509.Name:
    parts = [
        (NameOID.COUNTRY_NAME, args.country),
        (NameOID.STATE_OR_PROVINCE_NAME, args.state),
        (NameOID.LOCALITY_NAME, args.locality),
        (NameOID.ORGANIZATION_NAME, args.organization),
        (NameOID.COMMON_NAME, args.common_name),
    ]
    return x509.Name([x509.NameAttribute(oid, value) for oid, value in parts if value])


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--region", required=True, help="AWS region of the key")
    ap.add_argument("--key", required=True, help="key id, key ARN or alias (alias/grimalkin-codesign)")
    ap.add_argument("--common-name", required=True, help="subject CN (person or organisation name)")
    ap.add_argument("--organization", help="subject O (organisations)")
    ap.add_argument("--locality", help="subject L")
    ap.add_argument("--state", help="subject ST")
    ap.add_argument("--country", help="subject C (two letters)")
    ap.add_argument("--out", help="write the CSR here instead of standard output")
    args = ap.parse_args()
    if args.country and len(args.country) != 2:
        ap.error("--country must be a two-letter code")

    kms = boto3.client("kms", region_name=args.region)
    pub = kms.get_public_key(KeyId=args.key)
    if pub["KeyUsage"] != "SIGN_VERIFY" or pub["KeySpec"] not in ("RSA_3072", "RSA_4096"):
        raise SystemExit(f"key is {pub['KeySpec']}/{pub['KeyUsage']}; need an RSA_3072 or RSA_4096 SIGN_VERIFY key")
    if "RSASSA_PKCS1_V1_5_SHA_256" not in pub["SigningAlgorithms"]:
        raise SystemExit("the key does not support RSASSA_PKCS1_V1_5_SHA_256")

    def sign(tbs: bytes) -> bytes:
        # RAW message: KMS hashes it (a CSR body is far below the 4096-byte limit).
        return kms.sign(KeyId=args.key, Message=tbs, MessageType="RAW",
                        SigningAlgorithm="RSASSA_PKCS1_V1_5_SHA_256")["Signature"]

    csr = build_csr(pub["PublicKey"], make_subject(args), sign)

    # Belt and braces: the CSR must carry the KMS public key.
    mine = serialization.load_der_public_key(pub["PublicKey"]).public_numbers()
    theirs = csr.public_key().public_numbers()
    if mine != theirs:
        raise SystemExit("internal error: public key mismatch")

    pem = csr.public_bytes(serialization.Encoding.PEM).decode()
    if args.out:
        with open(args.out, "w") as fh:
            fh.write(pem)
    else:
        sys.stdout.write(pem)
    print(f"subject: {csr.subject.rfc4514_string()}  key: {pub['KeyId']} ({pub['KeySpec']})", file=sys.stderr)


if __name__ == "__main__":
    main()
