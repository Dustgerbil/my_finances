use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    Manager, Runtime,
};

tauri::ios_plugin_binding!(init_plugin_finance_sync);

pub struct FinanceSync<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> FinanceSync<R> {
    // Exposed only to Rust. No arbitrary filesystem paths are accepted from JS.
    pub fn connect(&self) -> Result<(), String> {
        self.0
            .run_mobile_plugin::<serde_json::Value>("connect", ())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn read_snapshot(&self, destination: &std::path::Path) -> Result<(), String> {
        self.0
            .run_mobile_plugin::<serde_json::Value>(
                "readSnapshot",
                serde_json::json!({"destination": destination.to_string_lossy()}),
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("finance-sync")
        .setup(|app, api| {
            let handle = api.register_ios_plugin(init_plugin_finance_sync)?;
            app.manage(FinanceSync(handle));
            Ok(())
        })
        .build()
}
