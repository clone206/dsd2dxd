<!-- markdownlint-disable MD033 -->
# <img src="doc/images/logo.png" width="100" alt="Logo"/> DSD Stream File #

[![pipeline status](https://gitlab.com/danieljrmay/dsf/badges/master/pipeline.svg)](https://gitlab.com/danieljrmay/dsf/commits/master)
[![Crate](https://img.shields.io/crates/v/dsf.svg)](https://crates.io/crates/dsf)
[![Documentation](https://docs.rs/dsf/badge.svg)](https://docs.rs/dsf/)

DSF (DSD Stream File) support in Rust. DSF files are a high-resolution
audio format that contain lossless 1-bit audio stream in delta sigma
modulation aka Direct Stream Digital (DSD). The format is intended for
1-bit DSD DACs.

This library is used by the
[`dsd`](https://gitlab.com/danieljrmay/dsd) project which provides
executables for inspecting and playing DSF files.

## References ##

* [DSF file format
specification](https://dsd-guide.com/sites/default/files/white-papers/DSFFileFormatSpec_E.pdf)
