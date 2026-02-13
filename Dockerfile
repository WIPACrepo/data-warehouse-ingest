# Stage 1: Build
FROM rust:trixie AS builder
WORKDIR /app
COPY . .

# set VERSION
ARG VERSION
ENV VERSION=$VERSION
RUN if [ -n "$VERSION" ]; then \
      if [ ! -f ~/.cargo/bin/cargo-set-version ]; then cargo install cargo-set-version; fi && \
      cargo set-version ${VERSION#v}; \
      echo "VERSION set to ${VERSION#v}"; \
    else \
      echo "VERSION not set, skipping cargo-set-version"; \
    fi

# automatically find binary targets and put them in /usr/local/cargo/bin/
RUN python3 -c 'import tomllib;[print(m) for m in tomllib.load(open("Cargo.toml", "rb"))["workspace"]["members"]]' \
    | xargs -I{} cargo install --path {}

# Stage 2: Runtime (Minimal glibc environment)
FROM gcr.io/distroless/cc-debian13
# Copy everything from the standard cargo bin directory
COPY --from=builder /usr/local/cargo/bin/ /usr/local/bin/
