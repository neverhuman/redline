FROM rust:1.95.0-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS fixture
ARG SOURCE_SHA
ARG PAUSE
RUN --mount=type=cache,id=redline-owner-custody-56a48e5bc0804d118b4840e6696c456f,target=/cache,sharing=locked printf 'fn main(){println!("{}",env!("SOURCE_SHA"));}' > /source.rs && SOURCE_SHA=$SOURCE_SHA rustc /source.rs -o /cache/redlinedb && echo compiled:$SOURCE_SHA && sleep $PAUSE && mkdir /out && cp /cache/redlinedb /out/redlinedb && /out/redlinedb > /out/source-sha && echo staged:$SOURCE_SHA:$(cat /out/source-sha)
FROM scratch
COPY --from=fixture /out/ /
