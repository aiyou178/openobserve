#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
if [[ $# -gt 1 || ( $# -eq 1 && "$1" != amd64 && "$1" != arm64 ) ]]; then
  echo "Usage: bash contrib/aiyou/build-oidc-image.sh [amd64|arm64]" >&2
  exit 2
fi
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  echo "Commit corresponding source before packaging; the checkout must be clean" >&2
  exit 1
fi
revision=$(git rev-parse HEAD)
version=$(tr -d '\r\n' < contrib/aiyou/VERSION)
toolchain_pin=$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)
toolchain=${OIDC_RUST_TOOLCHAIN:-$toolchain_pin}
jobs=${OIDC_BUILD_JOBS:-1}
requested_minimum_gb=${OIDC_MIN_FREE_GB:-}
stop_gb=5
if [[ ! "$version" =~ ^v1\.1\.2\.[1-9][0-9]*$ || ! "$toolchain" =~ ^nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "Expected version v1.1.2.N and a dated nightly-YYYY-MM-DD toolchain" >&2
  exit 1
fi
if [[ ! "$jobs" =~ ^[1-9][0-9]*$ || ( -n "$requested_minimum_gb" && ! "$requested_minimum_gb" =~ ^[1-9][0-9]*$ ) ]]; then
  echo "OIDC_BUILD_JOBS and any OIDC_MIN_FREE_GB override must be positive integers" >&2
  exit 1
fi
if [[ -n "$requested_minimum_gb" ]] && (( requested_minimum_gb < 16 )); then
  echo "OIDC_MIN_FREE_GB must be at least 16 for a cached native image build" >&2
  exit 1
fi
docker_context=desktop-linux
docker_endpoint=$(docker context inspect "$docker_context" --format '{{.Endpoints.docker.Host}}')
if [[ "$docker_endpoint" != unix://* || ( -n "${DOCKER_HOST:-}" && "$DOCKER_HOST" != "$docker_endpoint" ) ]]; then
  echo "Use Docker Desktop's local desktop-linux Unix socket; remote engines are not supported" >&2
  exit 1
fi
builder_driver=$(docker --context "$docker_context" buildx inspect "$docker_context" | awk '$1 == "Driver:" {print $2}')
if [[ "$builder_driver" != docker ]]; then
  echo "Use Docker Desktop's built-in local docker builder" >&2
  exit 1
fi
engine_arch=$(docker --context "$docker_context" info --format '{{.Architecture}}')
case "$engine_arch" in
  aarch64|arm64) native_arch=arm64 ;;
  x86_64|amd64) native_arch=amd64 ;;
  *) echo "Unsupported Docker engine architecture: $engine_arch" >&2; exit 1 ;;
esac
platform=${1:-$native_arch}
image="openobserve-oidc:$revision-$platform"
if docker --context "$docker_context" image inspect "$image" > /dev/null 2>&1; then
  echo "Refusing to overwrite existing image $image" >&2
  exit 1
fi
check_disk_space() {
  local floor_gb=$1 available_kb docker_available_kb
  available_kb=$(df -Pk "$PWD" | awk 'END {print $4}')
  docker_available_kb=$(docker --context "$docker_context" run --rm --pull missing --platform "linux/$native_arch" \
    --network none --read-only ubuntu:24.04 df -Pk / | awk 'END {print $4}')
  if [[ ! "$available_kb" =~ ^[0-9]+$ || ! "$docker_available_kb" =~ ^[0-9]+$ ]]; then
    echo "Could not measure host and Docker filesystem free space" >&2
    return 1
  fi
  if (( available_kb < floor_gb * 1024 * 1024 || docker_available_kb < floor_gb * 1024 * 1024 )); then
    echo "Insufficient disk space: host $((available_kb / 1024 / 1024)) GiB, Docker $((docker_available_kb / 1024 / 1024)) GiB; require $floor_gb GiB" >&2
    return 1
  fi
}
check_disk_space 16
archive_dir=$(mktemp -d "${TMPDIR:-/tmp}/openobserve-image.XXXXXX")
build_pid=
monitor_pid=
cancel_build() {
  local pid=$1 plugin_pid='' child_pid child_ppid child_command
  local verified_pid verified_ppid verified_command
  if [[ ! "$pid" =~ ^[0-9]+$ ]]; then
    return 0
  fi
  while read -r child_pid child_ppid child_command _; do
    if [[ "$child_pid" =~ ^[0-9]+$ && "$child_ppid" == "$pid" ]]; then
      child_command=${child_command##*/}
      if [[ "$child_command" == docker-buildx ]]; then
        plugin_pid=$child_pid
        break
      fi
    fi
  done < <(ps -axo pid=,ppid=,command=)
  if [[ -n "$plugin_pid" ]] && read -r verified_pid verified_ppid verified_command _ < <(
    ps -p "$plugin_pid" -o pid=,ppid=,command=
  ); then
    verified_command=${verified_command##*/}
    if [[ "$verified_pid" == "$plugin_pid" && "$verified_ppid" == "$pid" &&
      "$verified_command" == docker-buildx ]]; then
      kill "$plugin_pid" 2>/dev/null || true
    fi
  fi
  if read -r verified_pid verified_ppid verified_command _ < <(
    ps -p "$pid" -o pid=,ppid=,command=
  ); then
    verified_command=${verified_command##*/}
    if [[ "$verified_pid" == "$pid" && "$verified_command" == docker ]]; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
}
cleanup() {
  if [[ -n "$monitor_pid" ]]; then
    kill "$monitor_pid" 2>/dev/null || true
    wait "$monitor_pid" 2>/dev/null || true
  fi
  if [[ -n "$build_pid" ]]; then
    cancel_build "$build_pid"
    wait "$build_pid" 2>/dev/null || true
  fi
  rm -rf "$archive_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
minimum_gb=40
mkdir "$archive_dir/cachecheck"
if [[ "$platform" == "$native_arch" ]]; then
  minimum_gb=20
  if docker --context "$docker_context" buildx build --builder "$docker_context" --platform "linux/$platform" \
    --progress plain --target cachecheck --network none \
    --file "$PWD/contrib/aiyou/Dockerfile.binaries" \
    --build-arg "RUST_TOOLCHAIN=$toolchain" --build-arg "CACHE_PROBE_NONCE=${archive_dir##*/}" "$archive_dir/cachecheck"; then
    minimum_gb=16
  else
    echo "Native target cache not confirmed; using the 20 GiB native-build floor and retained download/layer caches" >&2
  fi
fi
if [[ -n "$requested_minimum_gb" ]]; then
  if (( requested_minimum_gb < minimum_gb )); then
    echo "OIDC_MIN_FREE_GB cannot be below the detected $minimum_gb GiB build floor" >&2
    exit 1
  fi
  minimum_gb=$requested_minimum_gb
fi
printf 'Disk guard: %s GiB start, %s GiB stop reserve; no cache deletion\n' "$minimum_gb" "$stop_gb"
check_disk_space "$minimum_gb"
source_name="openobserve-source-$revision-$version"
git archive --format=tar --prefix="$source_name/" "$revision" | gzip -n > "$archive_dir/$source_name.tar.gz"
mkdir "$archive_dir/context"
tar -xzf "$archive_dir/$source_name.tar.gz" --strip-components=1 -C "$archive_dir/context"
mkdir "$archive_dir/context/corresponding-source"
cp "$archive_dir/$source_name.tar.gz" "$archive_dir/context/corresponding-source/"
check_disk_space "$minimum_gb"
printf 'Building %s from %s with %s (%s jobs)\n' "$image" "$revision" "$toolchain" "$jobs"
docker --context "$docker_context" buildx build --builder "$docker_context" --platform "linux/$platform" --progress plain \
  --file "$archive_dir/context/contrib/aiyou/Dockerfile.binaries" \
  --target runtime --load --tag "$image" \
  --build-arg "VERSION=$version" \
  --build-arg "SOURCE_REVISION=$revision" \
  --build-arg "RUST_TOOLCHAIN=$toolchain" --build-arg "RUST_TOOLCHAIN_PIN=$toolchain_pin" \
  --build-arg "BUILD_JOBS=$jobs" "$archive_dir/context" &
build_pid=$!
monitor_disk_space() {
  while kill -0 "$build_pid" 2>/dev/null; do
    if ! check_disk_space "$stop_gb"; then
      echo "Stopping this image build to preserve the $stop_gb GiB disk reserve; no data was deleted" >&2
      cancel_build "$build_pid"
      return 1
    fi
    sleep 15
  done
}
monitor_disk_space &
monitor_pid=$!
build_status=0
wait "$build_pid" || build_status=$?
kill "$monitor_pid" 2>/dev/null || true
wait "$monitor_pid" 2>/dev/null || true
monitor_pid=
build_pid=
if (( build_status != 0 )); then
  exit "$build_status"
fi
test "$(docker --context "$docker_context" image inspect "$image" --format '{{.Os}}/{{.Architecture}}')" = "linux/$platform"
test "$(docker --context "$docker_context" image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')" = "$revision"
test "$(docker --context "$docker_context" image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.version"}}')" = "$version"
runtime_args=(--rm --platform "linux/$platform" --network none --read-only)
if [[ "$platform" == amd64 ]]; then
  runtime_args+=(--env QEMU_GUEST_BASE=0x800000000000)
fi
test "$(docker --context "$docker_context" run "${runtime_args[@]}" "$image" --version)" = "openobserve $version"
printf 'Image: %s\nSource commit: %s\n' "$image" "$revision"
docker --context "$docker_context" image inspect "$image" --format 'Image ID: {{.Id}}'
