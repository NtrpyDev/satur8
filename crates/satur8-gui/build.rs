fn main() {
    // Force the bundled "fluent" widget style so Button/ComboBox/Switch render
    // consistently everywhere (the auto-selected native style can leave default
    // buttons unpainted on some Linux setups).
    let config =
        slint_build::CompilerConfiguration::new().with_style("fluent-light".to_string());
    slint_build::compile_with_config("ui/app.slint", config).expect("compiling app.slint");
}
