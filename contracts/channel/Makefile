default: build

all: test

test: build
	cargo test

build:
	stellar contract build
	@ls -l target/wasm32v1-none/release/*.wasm

readme:
	cd contracts/channel \
		&& cargo +nightly rustdoc -- -Zunstable-options -wjson \
		&& cat ../../target/doc/channel.json \
		| jq -r '.index[.root|tostring].docs' \
		> README.md

fmt:
	cargo fmt --all

clean:
	cargo clean
