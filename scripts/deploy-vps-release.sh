#!/usr/bin/env bash
# Run on the VPS only AFTER GitHub Actions has built and published the archive.
# No cargo, pnpm, node build or compilation happens on the VPS.
set -Eeuo pipefail

tag="${1:-}"
if [[ ! "$tag" =~ ^deploy-vps-[a-zA-Z0-9._-]+$ ]]; then
  echo "Usage: bash scripts/deploy-vps-release.sh deploy-vps-<version>" >&2
  exit 2
fi

root=/opt/exameow
base="https://github.com/lllli223/exameow/releases/download/$tag"
artifact="exameow-vps-linux-x86_64.tar.gz"
tmp="$(mktemp -d "$root/tmp/vps-release.XXXXXX")"
stage="$(mktemp -d "$root/releases/.staging.XXXXXX")"
old="$(readlink -f "$root/current")"
switched=0

rollback() {
  if [[ "$switched" == 1 ]]; then
    echo "[deploy] Verification failed, rolling back to $old" >&2
    ln -s "$old" "$root/.current.rollback.$$"
    mv -Tf "$root/.current.rollback.$$" "$root/current"
    systemctl restart exameow.service || true
  fi
}
cleanup() {
  rm -rf "$tmp"
  if [[ -n "$stage" ]]; then rm -rf "$stage"; fi
}
trap rollback ERR
trap cleanup EXIT

echo "[deploy] Downloading precompiled release $tag"
curl -fLsS --retry 3 --connect-timeout 20 -o "$tmp/$artifact" "$base/$artifact"
curl -fLsS --retry 3 --connect-timeout 20 -o "$tmp/$artifact.sha256" "$base/$artifact.sha256"
(cd "$tmp" && sha256sum -c "$artifact.sha256")

# Guard the archive against absolute paths / parent-directory traversal.
if tar -tzf "$tmp/$artifact" | grep -Eq '^/|(^|/)\.\.(/|$)'; then
  echo "[deploy] Unsafe archive path" >&2
  exit 1
fi
tar -xzf "$tmp/$artifact" --no-same-owner -C "$stage"
test -x "$stage/exameow-server"
test -s "$stage/web/index.html"
test -s "$stage/web/ocr/recognition.onnx"
revision="$(tr -d '\r\n' < "$stage/REVISION")"
if [[ ! "$revision" =~ ^[a-f0-9]{40}$ ]]; then
  echo "[deploy] Invalid revision" >&2
  exit 1
fi
dest="$root/releases/${revision:0:7}"
if [[ -e "$dest" ]]; then
  echo "[deploy] Release path already exists: $dest" >&2
  exit 1
fi

echo "[deploy] Backing up SQLite before switching binaries"
export BACKUP_PATH="$root/backups/exameow-pre-cross-device-$(date +%Y%m%d-%H%M%S).db"
python3 - <<'PY'
import os, sqlite3
src = '/opt/exameow/data/exameow.db'
with sqlite3.connect('file:' + src + '?mode=ro', uri=True) as original:
    with sqlite3.connect(os.environ['BACKUP_PATH']) as backup:
        original.backup(backup)
        assert backup.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
os.chmod(os.environ['BACKUP_PATH'], 0o600)
print('[deploy] SQLite backup integrity OK')
PY

chmod -R a+rX "$stage"
mv "$stage" "$dest"
stage=''
ln -s "$dest" "$root/.current.next.$$"
mv -Tf "$root/.current.next.$$" "$root/current"
switched=1
systemctl restart exameow.service

echo "[deploy] Checking frontend and the authenticated progress API"
for attempt in {1..15}; do
  if curl -fsS --max-time 3 -o /dev/null http://127.0.0.1:31888/; then break; fi
  sleep 1
done
python3 - <<'PY'
import json, urllib.request
token = open('/opt/exameow/data/study_token', encoding='utf-8').read().strip()
req = urllib.request.Request('http://127.0.0.1:31888/api/study/progress',
                             headers={'Authorization': 'Bearer ' + token})
with urllib.request.urlopen(req, timeout=10) as response:
    assert response.status == 200
    assert isinstance(json.load(response)['progress'], list)
print('[deploy] Authenticated progress API OK')
PY
test "$(systemctl is-active exameow.service)" = active
test "$(readlink -f "$root/current")" = "$dest"
switched=0
trap - ERR
echo "[deploy] SUCCESS $tag ($revision)"
