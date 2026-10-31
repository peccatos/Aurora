fn main() {
    aurora_app::initialize_observability();

    aurora_app::compose()
        .run(tauri::generate_context!())
        .expect("failed to run Aurora desktop application");
}
