.PHONY: build test lint fmt fmt-check check clean

build:
	cargo build --release

test:
	cargo test

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

check: fmt-check lint test

clean:
	cargo clean
