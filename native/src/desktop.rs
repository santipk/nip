use eframe::egui::{Context, ViewportCommand};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

pub struct Desktop {
    _tray: TrayIcon,
    quit: Arc<AtomicBool>,
}
impl Desktop {
    pub fn new(ctx: &Context) -> Result<Self, String> {
        let menu = Menu::new();
        let open = MenuItem::new("Abrir NIP", true, None);
        let exit = MenuItem::new("Salir de NIP", true, None);
        menu.append(&open).map_err(|e| e.to_string())?;
        menu.append(&exit).map_err(|e| e.to_string())?;
        let data = eframe::icon_data::from_png_bytes(include_bytes!("../assets/nip-cat-32.png"))
            .map_err(|e| e.to_string())?;
        let icon =
            Icon::from_rgba(data.rgba, data.width, data.height).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("NIP · llamadas y chat")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| e.to_string())?;
        let quit = Arc::new(AtomicBool::new(false));
        let flag = quit.clone();
        let context = ctx.clone();
        let open_id = open.id().clone();
        let exit_id = exit.id().clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == exit_id {
                flag.store(true, Ordering::SeqCst);
            }
            if event.id == open_id || event.id == exit_id {
                context.send_viewport_cmd(ViewportCommand::Visible(true));
                context.send_viewport_cmd(ViewportCommand::Minimized(false));
                context.send_viewport_cmd(ViewportCommand::Focus);
                context.request_repaint();
            }
        }));
        let context = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                context.send_viewport_cmd(ViewportCommand::Visible(true));
                context.send_viewport_cmd(ViewportCommand::Minimized(false));
                context.send_viewport_cmd(ViewportCommand::Focus);
                context.request_repaint();
            }
        }));
        Ok(Self { _tray: tray, quit })
    }
    pub fn quitting(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }
}
pub fn configure_startup(enabled: bool, profile: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
            System::Registry::*,
        };
        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(Some(0)).collect()
        }
        let key_path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = wide("NIPNative");
        let exe = std::env::current_exe().map_err(|_| "No se pudo obtener la ubicación de NIP.")?;
        let command = format!("\"{}\" --data-dir \"{}\"", exe.display(), profile.display());
        let value = wide(&command);
        let mut key = std::ptr::null_mut();
        unsafe {
            let result = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_path.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if result != ERROR_SUCCESS {
                return Err(format!(
                    "Windows no permitió configurar el inicio automático ({result})."
                ));
            }
            let result = if enabled {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr() as *const u8,
                    (value.len() * 2) as u32,
                )
            } else {
                RegDeleteValueW(key, name.as_ptr())
            };
            RegCloseKey(key);
            if result != ERROR_SUCCESS && !(result == ERROR_FILE_NOT_FOUND && !enabled) {
                return Err(format!(
                    "No se pudo actualizar el inicio automático ({result})."
                ));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, profile);
        Err("Inicio automático disponible en Windows.".into())
    }
}
