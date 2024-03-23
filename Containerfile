FROM docker.io/rust:bookworm as builder
WORKDIR app
    
COPY . .
RUN cargo build --release --bin mollysocket


FROM docker.io/debian:bookworm-slim as runtime
WORKDIR app

ENV MOLLY_HOST=127.0.0.1
ENV MOLLY_PORT=8020

RUN apt update && \
    apt install -y wget libssl3 libsqlite3-0 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mollysocket /usr/local/bin/
HEALTHCHECK --interval=1m --timeout=3s \
    CMD wget -q --tries=1 "http://$MOLLY_HOST:$MOLLY_PORT/" -O - | grep '"mollysocket":{"version":'
ENTRYPOINT ["/usr/local/bin/mollysocket"]
