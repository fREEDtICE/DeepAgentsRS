#!/usr/bin/env bash
set -euo pipefail

OUTPUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      if [[ $# -lt 2 ]]; then
        echo "error: --output 需要一个目录参数" >&2
        exit 1
      fi
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --output=*)
      OUTPUT_DIR="${1#*=}"
      shift
      ;;
    *)
      echo "error: 未知参数: $1" >&2
      echo "usage: $0 --output <dir>" >&2
      exit 1
      ;;
  esac
done

if [[ -z "${OUTPUT_DIR}" ]]; then
  echo "error: --output 为必填参数" >&2
  echo "usage: $0 --output <dir>" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CLI_DIR="${REPO_ROOT}/crates/deepagents-cli"
DOC_FILE="${REPO_ROOT}/docs/design/cli-e2e-test-plan.md"
BIN_SOURCE="${REPO_ROOT}/target/debug/deepagents"
BIN_TARGET_NAME="deepagents-cli"
METADATA_FILE_NAME="metadata.txt"

if [[ ! -d "${CLI_DIR}" ]]; then
  echo "error: CLI 目录不存在: ${CLI_DIR}" >&2
  exit 1
fi

if [[ ! -f "${DOC_FILE}" ]]; then
  echo "error: 文档不存在: ${DOC_FILE}" >&2
  exit 1
fi

mkdir -p "${OUTPUT_DIR}"

cargo build -p deepagents-cli --manifest-path "${CLI_DIR}/Cargo.toml"

if [[ ! -f "${BIN_SOURCE}" ]]; then
  echo "error: 未找到编译产物: ${BIN_SOURCE}" >&2
  exit 1
fi

PACKAGE_TIME_UTC="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
PACKAGE_TIME_EPOCH="$(date -u +"%s")"
HEAD_COMMIT="$(git -C "${REPO_ROOT}" rev-parse HEAD)"
HEAD_SHORT="$(git -C "${REPO_ROOT}" rev-parse --short HEAD)"
BRANCH_NAME="$(git -C "${REPO_ROOT}" rev-parse --abbrev-ref HEAD)"
GIT_DIRTY="false"
if ! git -C "${REPO_ROOT}" diff --quiet || ! git -C "${REPO_ROOT}" diff --cached --quiet; then
  GIT_DIRTY="true"
fi
RUSTC_VERSION="$(rustc --version)"
CARGO_VERSION="$(cargo --version)"
HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"

cp "${BIN_SOURCE}" "${OUTPUT_DIR}/${BIN_TARGET_NAME}"
chmod +x "${OUTPUT_DIR}/${BIN_TARGET_NAME}"
cp "${DOC_FILE}" "${OUTPUT_DIR}/$(basename "${DOC_FILE}")"

cat > "${OUTPUT_DIR}/${METADATA_FILE_NAME}" <<EOF
package_time_utc=${PACKAGE_TIME_UTC}
package_time_epoch=${PACKAGE_TIME_EPOCH}
repo_root=${REPO_ROOT}
cli_dir=${CLI_DIR}
binary_source=${BIN_SOURCE}
binary_output=${OUTPUT_DIR}/${BIN_TARGET_NAME}
build_profile=debug
head_commit=${HEAD_COMMIT}
head_short=${HEAD_SHORT}
head_branch=${BRANCH_NAME}
git_dirty=${GIT_DIRTY}
rustc_version=${RUSTC_VERSION}
cargo_version=${CARGO_VERSION}
host_os=${HOST_OS}
host_arch=${HOST_ARCH}
doc_source=${DOC_FILE}
doc_output=${OUTPUT_DIR}/$(basename "${DOC_FILE}")
EOF

echo "打包完成: ${OUTPUT_DIR}"
echo " - 可执行文件: ${OUTPUT_DIR}/${BIN_TARGET_NAME}"
echo " - 元数据文件: ${OUTPUT_DIR}/${METADATA_FILE_NAME}"
echo " - 文档文件: ${OUTPUT_DIR}/$(basename "${DOC_FILE}")"
