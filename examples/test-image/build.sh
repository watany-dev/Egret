#!/usr/bin/env bash
# Build a minimal local Docker image for dog routing Level 3 tests.
#
# This image provides /bin/sh + basic utilities (grep, sleep, test, etc.)
# so healthCheck commands like "test -f /tmp/ready" work inside containers.
#
# The image has a default CMD that sleeps, so containers without an explicit
# command in the task definition will still run.
set -euo pipefail

IMAGE_NAME="${1:-egret-test:latest}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STAGING="$SCRIPT_DIR/.staging"

# Skip if the image already exists
if docker image inspect "$IMAGE_NAME" &>/dev/null; then
  echo "Image $IMAGE_NAME already exists, skipping build."
  exit 0
fi

echo "Building local test image: $IMAGE_NAME"

rm -rf "$STAGING"
mkdir -p "$STAGING/bin" "$STAGING/lib/x86_64-linux-gnu" "$STAGING/lib64" \
         "$STAGING/usr/bin" "$STAGING/etc" "$STAGING/tmp" "$STAGING/var/log/app"

# Copy essential binaries
for bin in sh dash sleep echo cat ls mkdir rm cp test true false grep head tail \
           wc tr sort tee printf date hostname; do
  for dir in /bin /usr/bin; do
    if [ -f "$dir/$bin" ]; then
      cp "$dir/$bin" "$STAGING/bin/" 2>/dev/null || true
      break
    fi
  done
done

# Resolve and copy all required shared libraries
for bin in "$STAGING"/bin/*; do
  [ -f "$bin" ] || continue
  ldd "$bin" 2>/dev/null | grep "=>" | awk '{print $3}' | while read -r lib; do
    [ -f "$lib" ] && cp -n "$lib" "$STAGING/lib/x86_64-linux-gnu/" 2>/dev/null || true
  done
done
# Dynamic linker
cp /lib64/ld-linux-x86-64.so.2 "$STAGING/lib64/" 2>/dev/null || true

# DNS resolution support
echo "hosts: files dns" > "$STAGING/etc/nsswitch.conf"

# Build the image
cat > "$STAGING/Dockerfile" << 'DEOF'
FROM scratch
COPY bin/ /bin/
COPY lib/ /lib/
COPY lib64/ /lib64/
COPY usr/ /usr/
COPY etc/ /etc/
COPY tmp/ /tmp/
COPY var/ /var/
CMD ["/bin/sh", "-c", "sleep 3600"]
DEOF

docker build -t "$IMAGE_NAME" "$STAGING"

# Cleanup staging
rm -rf "$STAGING"

echo "Built $IMAGE_NAME successfully."
