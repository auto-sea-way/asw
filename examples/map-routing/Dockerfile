FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p asw-cli

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/target/release/asw /usr/local/bin/asw
ENV ASW_GRAPH=/data/asw.graph
ENV ASW_HOST=0.0.0.0
ENV ASW_PORT=3000
EXPOSE 3000
VOLUME /data
ENTRYPOINT ["asw"]
CMD ["serve"]
