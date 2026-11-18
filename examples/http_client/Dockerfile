# syntax=docker/dockerfile:1

FROM airlance/conduit:latest AS builder
WORKDIR /app
COPY cli.cdt client.cdt server.cdt config.cdt pipeline.cdt main.cdt ./
RUN conduit build main.cdt -o /app/cdt-curl

FROM scratch
COPY --from=builder /app/cdt-curl /cdt-curl
COPY events.json notes.txt ./
EXPOSE 8080
ENTRYPOINT ["/cdt-curl"]
CMD ["--help"]
