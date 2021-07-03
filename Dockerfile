FROM rust:1.52.1-slim
RUN apt-get update && \
    apt-get install -y python3 git curl llvm clang libclang-dev gcc-arm-none-eabi libc6-dev-i386
RUN cargo install flip-link cargo-binutils
RUN rustup target add thumbv8m.main-none-eabi
RUN rustup component add llvm-tools-preview
RUN apt-get install -y wget make