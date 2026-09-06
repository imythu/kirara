FROM alpine:3.22

RUN apk add --no-cache ca-certificates

COPY kirara /kirara
COPY assets/search/models/mini/LICENSE assets/search/models/mini/provenance.json /usr/share/doc/kirara/search/
ENV KIRARA_HOST=0.0.0.0
ENV KIRARA_PORT=3000
ENV KIRARA_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 3000
ENTRYPOINT ["/kirara"]
