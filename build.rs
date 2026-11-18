fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    for library in [
        "avformat",
        "avcodec",
        "avutil",
        "swscale",
        "swresample",
        "asound",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("av.*")
        .allowlist_function("sws_.*")
        .allowlist_function("swr_.*")
        .allowlist_function("snd_pcm_.*")
        .allowlist_type("AV.*")
        .allowlist_type("snd_pcm.*")
        .allowlist_var("AV_.*")
        .allowlist_var("SND_PCM_.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("failed to generate FFmpeg/ALSA bindings");
    bindings
        .write_to_file(
            std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bindings.rs"),
        )
        .expect("failed to write bindings");
}
