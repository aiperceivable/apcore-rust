.PHONY: setup check check-chars fmt-check lint build test build-examples fmt clean

# One-time dev environment setup
setup:
	@echo "Installing apdev-rs..."
	@command -v apdev-rs >/dev/null 2>&1 || cargo install apdev-rs
	@echo "Installing git pre-commit hook..."
	@mkdir -p .git/hooks
	@cp .githooks/pre-commit .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "Done! Development environment is ready."

# Run all checks (same as pre-commit hook, mirrors CI)
check: fmt-check lint check-chars build test build-examples

check-chars:
	apdev-rs check-chars src/

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

build:
	cargo build --all-features

test:
	cargo test --all-features

build-examples:
	cargo build --examples

fmt:
	cargo fmt --all

clean:
	cargo clean
