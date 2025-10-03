//! Qt user interface bootstrap for OpenMeters.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

/// Launch the Qt event loop and display the placeholder window.
pub fn run() -> Result<()> {
    let mut app = QGuiApplication::new();
    if app.is_null() {
        return Err(anyhow!("failed to initialise QGuiApplication"));
    }

    let mut engine = QQmlApplicationEngine::new();
    if engine.is_null() {
        return Err(anyhow!("failed to initialise QQmlApplicationEngine"));
    }

    let qml_path = qml_entry_point();
    let qml_qstring = QString::from(qml_path.to_string_lossy().as_ref());
    let qml_url = QUrl::from_local_file(&qml_qstring);

    engine.pin_mut().load(&qml_url);

    let exit_code = app.pin_mut().exec();
    if exit_code != 0 {
        Err(anyhow!("Qt event loop exited with status {exit_code}"))
    } else {
        Ok(())
    }
}

fn qml_entry_point() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/qml/main.qml")
}
