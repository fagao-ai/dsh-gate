#!/usr/bin/env bash
# Recreate the dsh-gate container with new login credentials.
# Usage: ./set-password.sh
set -euo pipefail

read -rp "New username (default: hezz): " user
user=${user:-hezz}
read -rsp "New password: " pass
echo
if [ -z "$pass" ]; then
  echo "password cannot be empty" >&2
  exit 1
fi

docker rm -f dsh-gate >/dev/null 2>&1 || true
docker run -d --name dsh-gate --restart unless-stopped -p 3081:8080 \
  -e "AUTH_USER=$user" \
  -e "AUTH_PASSWORD=$pass" \
  -e BACKEND=http://host.docker.internal:3080 \
  dsh-gate
echo "dsh-gate recreated — login with $user at https://dsh.hezz.eu.org"
