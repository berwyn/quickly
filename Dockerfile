FROM alpine:3.16.2 AS builder
WORKDIR /app

RUN apk add --no-cache \
    build-base \
    curl-dev \
    openssl-dev \
    rustup \
    libwebp

COPY rust-toolchain ./
RUN cat rust-toolchain | xargs rustup-init -yq --profile minimal --default-toolchain

COPY . ./
RUN $HOME/.cargo/bin/cargo build --release

FROM alpine:3.16.2

RUN apk add --no-cache \
    ca-certificates \
    curl-dev \
    openssl

COPY --from=builder /app/target/release/quickly /bin/

CMD ["quickly"]