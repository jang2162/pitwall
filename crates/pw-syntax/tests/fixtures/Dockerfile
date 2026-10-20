# A comment.
FROM rust:1.99 AS build
ARG PROFILE=release
WORKDIR /src
COPY . .
RUN cargo build --profile "$PROFILE" \
    && strip target/release/pitwall
ENV PATH="/usr/local/bin:${PATH}"
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pitwall", "--headless"]
