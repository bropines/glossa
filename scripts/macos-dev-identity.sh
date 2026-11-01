#!/bin/sh
# Create a self-signed code-signing identity, "Glossa Dev", in the login
# keychain, so local builds of Glossa.app keep one signature and macOS keeps
# their Accessibility and Screen Recording permissions across rebuilds (an
# ad-hoc signature changes with every build, and macOS then forgets them).
#
# Run it once, in Terminal on the Mac itself: macOS asks for the keychain
# password, which an SSH session cannot answer. Then sign with
#   codesign --force --deep -s "Glossa Dev" Glossa.app
# (scripts/macos-bundle.sh does that by itself when the identity exists).
set -e
NAME="Glossa Dev"
if security find-identity -p codesigning 2>/dev/null | grep -q "\"$NAME\""; then
  echo "The \"$NAME\" identity already exists."
  exit 0
fi
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT
cat > "$D/cfg" <<EOF
[req]
distinguished_name = dn
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$D/key.pem" -out "$D/cert.pem" \
  -days 3650 -config "$D/cfg" -extensions ext 2>/dev/null
PASS=$(openssl rand -hex 12)
# Older macOS `security` reads only the legacy PKCS#12 encryption.
openssl pkcs12 -export -legacy -inkey "$D/key.pem" -in "$D/cert.pem" -name "$NAME" \
  -out "$D/id.p12" -passout "pass:$PASS" 2>/dev/null ||
  openssl pkcs12 -export -inkey "$D/key.pem" -in "$D/cert.pem" -name "$NAME" \
    -out "$D/id.p12" -passout "pass:$PASS"
security import "$D/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "$PASS" -T /usr/bin/codesign
# Trust it for code signing, so codesign accepts it without complaint.
security add-trusted-cert -p codeSign -k "$HOME/Library/Keychains/login.keychain-db" "$D/cert.pem"
echo "Created \"$NAME\". Rebuild and re-bundle Glossa.app, then grant its"
echo "permissions one last time; they will now survive rebuilds."
