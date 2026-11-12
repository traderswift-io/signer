#!/bin/bash
set -euo pipefail
chain_directory="$(cd "$(dirname "$0")" && pwd)"
toolchain_gopath="$(go env GOPATH)"
export PATH="$toolchain_gopath/bin:$PATH"
software_development_kit_proto="$toolchain_gopath/pkg/mod/github.com/cosmos/cosmos-sdk@v0.55.0/proto"
workspace="$chain_directory/build/proto_workspace"

rm -rf "$workspace"
mkdir -p "$workspace/signer/policy/v1" "$workspace/signer/validatorset/v1" "$workspace/cosmos/msg/v1"
cp "$chain_directory/proto/policy.proto" "$workspace/signer/policy/v1/"
cp "$chain_directory/proto/validatorset.proto" "$workspace/signer/validatorset/v1/"
cp "$software_development_kit_proto/cosmos/msg/v1/msg.proto" "$workspace/cosmos/msg/v1/"
chmod -R u+w "$workspace"
cat > "$workspace/buf.gen.yaml" <<'YAML'
version: v2
plugins:
  - local: protoc-gen-gocosmos
    out: generated
    opt:
      - plugins=grpc
      - paths=source_relative
YAML
cd "$workspace"
buf generate --path signer/policy/v1/policy.proto --path signer/validatorset/v1/validatorset.proto
cp generated/signer/policy/v1/policy.pb.go "$chain_directory/policy/policy.pb.go"
mkdir -p "$chain_directory/validatorset"
cp generated/signer/validatorset/v1/validatorset.pb.go "$chain_directory/validatorset/validatorset.pb.go"
