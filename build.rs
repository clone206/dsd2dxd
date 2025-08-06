fn main() {
    cc::Build::new()
        .files(&[
            "dsd2pcm.c",
            "dsdin.c",
            "dsdiff.c",
            "dsf.c"
        ])
        .compile("dsd2pcm");

    // Tell cargo to invalidate the built crate whenever any of these change
    println!("cargo:rerun-if-changed=dsd2pcm.c");
    println!("cargo:rerun-if-changed=dsdin.c");
    println!("cargo:rerun-if-changed=dsdiff.c");
    println!("cargo:rerun-if-changed=dsf.c");
}