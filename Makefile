default: build

all: build test fmt readme

build:
	stellar contract build
	@ls -l target/wasm32v1-none/release/*.wasm

test: build
	cargo test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

doc:
	cargo doc --no-deps --open

doc-check:
	cargo doc --no-deps

readme:
	cd contracts/channel \
		&& cargo +nightly rustdoc -- -Zunstable-options -wjson
	cd contracts/channel-factory \
		&& cargo +nightly rustdoc -- -Zunstable-options -wjson
	jq -r '.index[.root|tostring].docs' target/doc/channel.json > README.md
	echo "" >> README.md
	jq -r '.index[.root|tostring].docs' target/doc/channel_factory.json >> README.md

readme-check: readme
	git add -N . && git diff HEAD --exit-code

install-tool-ed25519:
	cargo install --locked --path tools/ed25519

clean:
	cargo clean
