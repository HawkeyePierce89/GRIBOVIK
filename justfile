# Build the SPA, then the binary that embeds it.
default: build

# Install frontend dependencies and produce web/dist.
build-web:
    cd web && npm ci && npm run build

# A release binary with the frontend embedded. build-web must run first:
# build.rs refuses to compile without web/dist/index.html.
build: build-web
    cargo build --release

# The whole suite, both sides.
test:
    cargo test
    cd web && npm test
