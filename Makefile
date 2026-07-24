.PHONY: check fmt clippy test build manifest snapshots docs icon-generator

check: fmt clippy test build manifest snapshots docs icon-generator

fmt:
	mise exec -- cargo fmt --check

clippy:
	mise exec -- cargo clippy --all-targets --all-features -- -D warnings

test:
	mise exec -- cargo test --all-features

build:
	mise exec -- cargo build --release --locked

manifest:
	mise exec -- cargo run --quiet -- validate-manifest

snapshots:
	mise exec -- cargo test --all-features --test visual_system_snapshots --test visual_contract_matrix

docs:
	mise exec -- cargo test --all-features --test documentation_contract

icon-generator:
	mise exec -- rustfmt --edition 2024 --check tools/generate_icon_data.rs
