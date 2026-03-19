default: build

all: test

test: build
	cargo test

build:
	stellar contract build
	@ls -l target/wasm32v1-none/release/*.wasm

readme:
	cd contracts/channel \
		&& cargo +nightly rustdoc -- -Zunstable-options -wjson
	cd contracts/channel-factory \
		&& cargo +nightly rustdoc -- -Zunstable-options -wjson
	jq -r '.index[.root|tostring].docs' target/doc/channel.json > README.md
	echo "" >> README.md
	jq -r '.index[.root|tostring].docs' target/doc/channel_factory.json >> README.md

fmt:
	cargo fmt --all

clean:
	cargo clean
