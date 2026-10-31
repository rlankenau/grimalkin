#!/usr/bin/env bash
# Assemble signed apt and dnf repositories from built .deb and .rpm packages.
#
#   build-repo.sh REPO_DIR DEB_DIR RPM_DIR SUITE KEY_ID
#
#   REPO_DIR  the tree to add to (the contents of the gh-pages branch, or an
#             empty directory); new packages are added next to what is there
#   SUITE     stable (final releases) or testing (pre-releases)
#   KEY_ID    fingerprint of the secret key in the current GNUPGHOME
#
# Environment: GPG_PASSPHRASE (the key's passphrase, may be empty) and BASE_URL
# (where REPO_DIR will be served; written into the .repo files).
#
# Result:
#   apt/dists/SUITE/{Release,InRelease,Release.gpg,main/binary-*/Packages*}
#   apt/pool/SUITE/*.deb
#   rpm/<arch>/ (stable) or rpm-testing/<arch>/ (testing) with signed repodata
#   grimalkin.gpg (binary key for apt signed-by), grimalkin.asc (armored key),
#   grimalkin.repo, grimalkin-testing.repo
#
# Needs apt-ftparchive (apt-utils), createrepo_c, gpg, gzip, xz.
set -euo pipefail

die() { echo "build-repo: $*" >&2; exit 1; }
[[ $# -eq 5 ]] || die "usage: $0 REPO_DIR DEB_DIR RPM_DIR SUITE KEY_ID"
REPO=$(mkdir -p "$1" && cd "$1" && pwd)
DEBS=$2 RPMS=$3 SUITE=$4 KEYID=$5
BASE_URL=${BASE_URL:-https://rlankenau.github.io/grimalkin}

case "$SUITE" in
  stable) RPM_SUB=rpm ;;
  testing) RPM_SUB=rpm-testing ;;
  *) die "SUITE must be stable or testing, not '$SUITE'" ;;
esac
ls "$DEBS"/*.deb >/dev/null 2>&1 || die "no .deb files in $DEBS"
ls "$RPMS"/*.rpm >/dev/null 2>&1 || die "no .rpm files in $RPMS"

gpgsign() {
  gpg --batch --yes --pinentry-mode loopback --passphrase-fd 3 --local-user "$KEYID" "$@" 3<<<"${GPG_PASSPHRASE:-}"
}

# ---------------------------------------------------------------- apt
APT="$REPO/apt"
DIST="$APT/dists/$SUITE"
ARCHES="amd64 arm64"
mkdir -p "$APT/pool/$SUITE"
cp -f "$DEBS"/*.deb "$APT/pool/$SUITE/"
rm -rf "$DIST"
for arch in $ARCHES; do
  d="$DIST/main/binary-$arch"
  mkdir -p "$d"
  (cd "$APT" && apt-ftparchive --arch "$arch" packages "pool/$SUITE") > "$d/Packages"
  gzip -9 -n -c "$d/Packages" > "$d/Packages.gz"
  xz -9 -c "$d/Packages" > "$d/Packages.xz"
done
# Written outside DIST first so the Release file does not list itself.
release_tmp=$(mktemp)
(cd "$APT" && apt-ftparchive \
  -o "APT::FTPArchive::Release::Origin=Grimalkin" \
  -o "APT::FTPArchive::Release::Label=Grimalkin" \
  -o "APT::FTPArchive::Release::Suite=$SUITE" \
  -o "APT::FTPArchive::Release::Codename=$SUITE" \
  -o "APT::FTPArchive::Release::Architectures=$ARCHES" \
  -o "APT::FTPArchive::Release::Components=main" \
  -o "APT::FTPArchive::Release::Description=Grimalkin terminal emulator ($SUITE)" \
  release "dists/$SUITE") > "$release_tmp"
mv "$release_tmp" "$DIST/Release"
gpgsign --clearsign -o "$DIST/InRelease" "$DIST/Release"
gpgsign --armor --detach-sign -o "$DIST/Release.gpg" "$DIST/Release"

# ---------------------------------------------------------------- rpm
for rpm in "$RPMS"/*.rpm; do
  arch=${rpm%.rpm}
  arch=${arch##*.}
  mkdir -p "$REPO/$RPM_SUB/$arch"
  cp -f "$rpm" "$REPO/$RPM_SUB/$arch/"
done
for d in "$REPO/$RPM_SUB"/*/; do
  createrepo_c --quiet --retain-old-md 0 "$d"
  # repo_gpgcheck=1 verifies this signature; the repodata carries the package
  # checksums, so the packages are covered through it.
  gpgsign --armor --detach-sign -o "$d/repodata/repomd.xml.asc" "$d/repodata/repomd.xml"
done

# ---------------------------------------------------------------- keys, .repo
gpg --export "$KEYID" > "$REPO/grimalkin.gpg"
gpg --armor --export "$KEYID" > "$REPO/grimalkin.asc"
[[ -s "$REPO/grimalkin.gpg" ]] || die "could not export the public key"

write_repo() { # file id name subdir
  cat > "$REPO/$1" <<REPOEOF
[$2]
name=$3
baseurl=$BASE_URL/$4/\$basearch
enabled=1
gpgcheck=0
repo_gpgcheck=1
gpgkey=$BASE_URL/grimalkin.asc
REPOEOF
}
write_repo grimalkin.repo grimalkin "Grimalkin" rpm
write_repo grimalkin-testing.repo grimalkin-testing "Grimalkin (pre-releases)" rpm-testing

if [[ ! -e "$REPO/index.html" ]]; then
  cat > "$REPO/index.html" <<'HTMLEOF'
<!doctype html>
<meta charset="utf-8">
<title>Grimalkin package repositories</title>
<h1>Grimalkin package repositories</h1>
<p>These are the apt and dnf repositories for
<a href="https://github.com/rlankenau/grimalkin">Grimalkin</a>. Setup
instructions are in the README, section "Installing on Linux".</p>
HTMLEOF
fi
echo "build-repo: $SUITE repository assembled in $REPO"
