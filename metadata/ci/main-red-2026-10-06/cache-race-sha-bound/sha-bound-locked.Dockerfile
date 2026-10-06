FROM rust:1.95.0-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS fixture
ARG SOURCE_SHA
ARG PAUSE
ENV FIXTURE_OUTPUT=/out/${SOURCE_SHA}
RUN --mount=type=cache,id=redline-owner-custody-49ca0af12c614fa09e41e41f48dab542,target=/cache,sharing=locked printf 'fn main(){println!("{}",env!("SOURCE_SHA"));}' > /source.rs && SOURCE_SHA=$SOURCE_SHA rustc /source.rs -o /cache/redlinedb && echo compiled:$SOURCE_SHA && sleep $PAUSE && mkdir -p "$FIXTURE_OUTPUT" && cp /cache/redlinedb "$FIXTURE_OUTPUT/redlinedb" && "$FIXTURE_OUTPUT/redlinedb" > "$FIXTURE_OUTPUT/source-sha" && echo staged:$SOURCE_SHA:$(cat "$FIXTURE_OUTPUT/source-sha")
FROM scratch
ARG SOURCE_SHA
COPY --from=fixture /out/${SOURCE_SHA}/ /
