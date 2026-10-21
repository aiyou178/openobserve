#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
case "${1:-amd64}" in
  amd64|arm64) platforms=("${1:-amd64}") ;;
  all) platforms=(amd64 arm64) ;;
  *) echo "Usage: bash contrib/aiyou/build-binaries.sh [amd64|arm64|all]" >&2; exit 2 ;;
esac
if [[ $# -gt 1 ]]; then
  echo "Usage: bash contrib/aiyou/build-binaries.sh [amd64|arm64|all]" >&2
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
minimum_gb=${OIDC_MIN_FREE_GB:-40}
if [[ ! "$version" =~ ^v1\.1\.2\.[1-9][0-9]*$ || ! "$toolchain" =~ ^nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "Expected version v1.1.2.N and a dated nightly-YYYY-MM-DD toolchain" >&2
  exit 1
fi
if [[ ! "$jobs" =~ ^[1-9][0-9]*$ || ! "$minimum_gb" =~ ^[1-9][0-9]*$ ]]; then
  echo "OIDC_BUILD_JOBS and OIDC_MIN_FREE_GB must be positive integers" >&2
  exit 1
fi
docker_context=$(docker context show)
docker_endpoint=$(docker context inspect "$docker_context" --format '{{.Endpoints.docker.Host}}')
if [[ "$docker_endpoint" != unix://* || ( -n "${DOCKER_HOST:-}" && "$DOCKER_HOST" != "$docker_endpoint" ) ]]; then
  echo "Select a local Docker context and unset any conflicting DOCKER_HOST; remote engines are not supported" >&2
  exit 1
fi
docker --context "$docker_context" info > /dev/null
docker --context "$docker_context" buildx version
if [[ "${1:-amd64}" == all && "$(docker --context "$docker_context" info --format '{{.Architecture}}')" == aarch64 ]]; then
  platforms=(arm64 amd64)
fi
output_dir=${OIDC_BUILD_OUTPUT_DIR:-"$PWD/bin/oidc"}
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd -P)
available_kb=$(df -Pk "$output_dir" | awk 'END {print $4}')
if (( available_kb < minimum_gb * 1024 * 1024 )); then
  echo "Insufficient host disk space: $((available_kb / 1024 / 1024)) GiB free; require at least $minimum_gb GiB" >&2
  echo "Free space on the Docker data filesystem and output filesystem before building; no files were deleted" >&2
  exit 1
fi
archive_dir=$(mktemp -d "${TMPDIR:-/tmp}/openobserve-binaries.XXXXXX")
trap 'rm -rf "$archive_dir"' EXIT
source_name="openobserve-source-$revision-$version"
git archive --format=tar --prefix="$source_name/" "$revision" | gzip -n > "$archive_dir/$source_name.tar.gz"
mkdir "$archive_dir/context"
tar -xzf "$archive_dir/$source_name.tar.gz" --strip-components=1 -C "$archive_dir/context"
mkdir "$archive_dir/context/corresponding-source"
cp "$archive_dir/$source_name.tar.gz" "$archive_dir/context/corresponding-source/"
for platform in "${platforms[@]}"; do
  available_kb=$(df -Pk "$output_dir" | awk 'END {print $4}')
  if (( available_kb < minimum_gb * 1024 * 1024 )); then
    echo "Insufficient disk space before $platform build; need at least $minimum_gb GiB free" >&2
    exit 1
  fi
  package_name="openobserve-linux-$platform-$version"
  if [[ -e "$output_dir/$package_name.tar.gz" || -e "$output_dir/$package_name.tar.gz.sha256" ]]; then
    echo "Refusing to overwrite $output_dir/$package_name.tar.gz or its checksum" >&2
    exit 1
  fi
  printf 'Building %s from %s with %s (%s jobs)\n' "$package_name" "$revision" "$toolchain" "$jobs"
  docker --context "$docker_context" buildx build --builder "$docker_context" --platform "linux/$platform" --progress plain \
    --file "$archive_dir/context/contrib/aiyou/Dockerfile.binaries" \
    --target artifacts --output "type=local,dest=$output_dir" \
    --build-arg "VERSION=$version" --build-arg "SOURCE_REVISION=$revision" \
    --build-arg "RUST_TOOLCHAIN=$toolchain" --build-arg "RUST_TOOLCHAIN_PIN=$toolchain_pin" \
    --build-arg "BUILD_JOBS=$jobs" "$archive_dir/context"
  (cd "$output_dir" && shasum -a 256 --check "$package_name.tar.gz.sha256")
done
printf 'Packages: %s\nSource commit: %s\n' "$output_dir" "$revision"
