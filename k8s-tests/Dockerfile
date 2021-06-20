ARG RUST_IMAGE=rust:1.53.0-buster
ARG RUNTIME_IMAGE=gcr.io/distroless/cc:nonroot

FROM $RUST_IMAGE as build
WORKDIR /build
COPY . .
RUN --mount=type=cache,from=rust:1.53.0-buster,source=/usr/local/cargo,target=/usr/local/cargo \
	(cd k8s-tests && cargo build) && \
	mv target/debug/linkerd-k8s-tests /out/linkerd-k8s-tests

FROM $RUNTIME_IMAGE as runtime
COPY --from=build /out/linkerd-k8s-tests /usr/local/bin/linkerd-k8s-tests
ENV RUST_LOG=debug
ENTRYPOINT ["/usr/local/bin/linkerd-k8s-tests"]
