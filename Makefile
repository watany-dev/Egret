.PHONY: build test lint fmt fmt-check check clean coverage audit deny doc

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

coverage:
	cargo tarpaulin --out html --out json \
		--skip-clean \
		--fail-under 95 \
		--exclude-files "src/main.rs" \
		--timeout 300 \
		-- --test-threads=1

audit:
	cargo deny check advisories

deny:
	cargo deny check licenses bans sources

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

check: fmt-check lint test doc deny

clean:
	cargo clean
