fn main() {
    println!("Building cuda source files for GEM...");
    println!("cargo:rerun-if-changed=csrc");

    #[cfg(feature = "cuda")] {
        // Portable GPU-architecture defaults.
        //
        // ucc's built-in defaults (compute_50 PTX + sm_70/sm_80 SASS) were
        // removed from nvcc in CUDA 13, so a fresh clone fails to compile on
        // any CUDA 13 toolchain before reaching our code. If the user has not
        // chosen an architecture, emit SASS for every mainstream architecture
        // since Turing and embed compute_75 PTX so newer GPUs (e.g. Blackwell
        // sm_120) JIT-compile transparently on first launch.
        //
        // Override for a native build:  UCC_CUDA_GENCODE=120 UCC_CUDA_PTX=120
        // (RTX 50xx),  90 (H100),  89 (RTX 40xx),  86 (RTX 30xx / A100=80).
        if std::env::var_os("UCC_CUDA_GENCODE").is_none() {
            std::env::set_var("UCC_CUDA_GENCODE", "75,80,86,89,90");
        }
        if std::env::var_os("UCC_CUDA_PTX").is_none() {
            std::env::set_var("UCC_CUDA_PTX", "75");
        }

        let csrc_headers = ucc::import_csrc();
        let mut cl_cuda = ucc::cl_cuda();
        cl_cuda.ccbin(false);
        cl_cuda.flag("-lineinfo");
        cl_cuda.flag("-maxrregcount=128");
        cl_cuda.debug(false).opt_level(3)
            .include(&csrc_headers)
            .files(["csrc/kernel_v1.cu"]);
        cl_cuda.compile("gemcu");
        println!("cargo:rustc-link-lib=static=gemcu");
        println!("cargo:rustc-link-lib=dylib=cudart");
        ucc::bindgen(["csrc/kernel_v1.cu"], "kernel_v1.rs");
        ucc::export_csrc();
        ucc::make_compile_commands(&[&cl_cuda]);
    }
}
