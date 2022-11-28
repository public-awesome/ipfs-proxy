setup_dev:
	sea-orm-cli migrate init
	cargo install cargo-udeps

migrate:
	cargo run --bin migrate

run:
	RUST_LOG=info,sqlx::query=warn cargo run --bin httpd --release

clean:
	rm -rf ipfs objects.sqlite
