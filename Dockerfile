FROM gcr.io/distroless/static-debian12
ARG TARGETARCH
COPY asw-linux-${TARGETARCH} /usr/local/bin/asw
ENV ASW_GRAPH=/data/asw.graph
ENV ASW_HOST=0.0.0.0
ENV ASW_PORT=3000
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=120s \
  CMD ["/usr/local/bin/asw", "healthcheck"]
VOLUME /data
ENTRYPOINT ["asw"]
CMD ["serve"]
