FROM alpine:3.22

RUN apk add --no-cache ca-certificates

COPY kirara /kirara
ENV KIRARA_HOST=0.0.0.0
ENV KIRARA_PORT=3000
ENV KIRARA_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 3000
ENTRYPOINT ["/kirara"]
