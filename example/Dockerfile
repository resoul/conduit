# syntax=docker/dockerfile:1

FROM airlance/conduit:0.2.5 AS builder
WORKDIR /app
COPY config.cdt pipeline.cdt main.cdt ./
RUN conduit build main.cdt -o /app/worker

FROM scratch
COPY --from=builder /app/worker /worker
COPY events.json notes.txt ./
ENTRYPOINT ["/worker"]
CMD ["--help"]
