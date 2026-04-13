.PHONY: build build-web build-server dev dev-web dev-server clean

# Build everything: frontend first (so Rust can embed it), then server
build: build-web build-server

build-web:
	cd web && pnpm install && pnpm build

build-server: build-web
	cd server-rs && cargo build --release

dev:
	@echo "Starting dev mode (Node server + Vite)..."
	pnpm dev

dev-web:
	cd web && pnpm dev

dev-server:
	cd server-rs && cargo run

clean:
	rm -rf web/dist
	cd server-rs && cargo clean
