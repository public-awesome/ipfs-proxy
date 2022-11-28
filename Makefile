setup_dev:
	sea-orm-cli migrate init

migrate:
	cargo run --bin migrate
