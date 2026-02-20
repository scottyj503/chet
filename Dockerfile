FROM alpine:3.21

RUN apk add --no-cache ca-certificates

RUN adduser -D -h /home/chet chet
USER chet
WORKDIR /home/chet

COPY chet /usr/local/bin/chet

ENTRYPOINT ["chet"]
CMD ["-p", "hello"]
