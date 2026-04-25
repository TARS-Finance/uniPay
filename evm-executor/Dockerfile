# syntax=docker/dockerfile:1
# Build stage
FROM rust:latest AS builder
WORKDIR /usr/src/app

# Install cargo-chef for better dependency caching
RUN cargo install cargo-chef

# Install required tools and dependencies
RUN apt-get update && apt-get install -y \
    curl \
    jq \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

ARG GHPAT
ENV GHPAT=${GHPAT}

# Configure git with token
RUN git config --global url."https://${GHPAT}@github.com/".insteadOf "https://github.com/"

# Configure cargo to use git CLI for fetching
RUN mkdir -p ~/.cargo && \
    echo -e '[net]\ngit-fetch-with-cli = true' > ~/.cargo/config.toml

# === CARGO CHEF OPTIMIZATION ===
# First stage: Generate recipe
FROM builder AS planner
COPY Cargo.toml Cargo.lock ./
# Create minimal src structure for cargo-chef to understand the project
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

# Second stage: Build dependencies using recipe
FROM builder AS dependencies
COPY --from=planner /usr/src/app/recipe.json recipe.json

# Build dependencies with cache mount
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/app/target \
    cargo chef cook --release --recipe-path recipe.json

# Final build stage: Build the application
FROM dependencies AS build
COPY . .

# Build the actual application with cache mounts
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/app/target \
    cargo build --release && \
    cp target/release/evm-executor /usr/src/app/binary

# Runtime stage - use distroless for smaller size and better security
FROM gcr.io/distroless/cc-debian12
WORKDIR /app

# Copy the binary from the builder stage
COPY --from=build /usr/src/app/binary /app/binary

# Set the command to run the application
ENTRYPOINT ["/app/binary"]
