use crate::contracts::*;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

const BG: Color32 = Color32::from_rgb(17, 23, 21);
const SIDE: Color32 = Color32::from_rgb(25, 33, 28);
const TEXT: Color32 = Color32::from_rgb(237, 241, 232);
const SECONDARY: Color32 = Color32::from_rgb(155, 169, 159);
const ACCENT: Color32 = Color32::from_rgb(196, 237, 145);
const SELECTED: Color32 = Color32::from_rgb(49, 68, 44);
const ERROR: Color32 = Color32::from_rgb(255, 163, 156);
fn hotkey_label(key: u16) -> String {
    match key {
        0x05 => "Mouse 4".into(),
        0x06 => "Mouse 5".into(),
        _ => format!("F{}", key.saturating_sub(0x70) + 1),
    }
}

fn equalizer_settings(ui: &mut egui::Ui, title: &str, id: &str, settings: &mut EqualizerSettings) {
    egui::CollapsingHeader::new(title).id_salt(id).show(ui, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 168.0).max(120.0);
        ui.checkbox(&mut settings.enabled, "Activar ecualización");
        ui.add_enabled_ui(settings.enabled, |ui| {
            ui.label("Preamplificación");
            ui.add(egui::Slider::new(&mut settings.preamp_db, -24.0..=12.0).suffix(" dB").fixed_decimals(1));
            ui.label(RichText::new("10 bandas · Q 1,4").strong());
            egui::Grid::new(id).num_columns(2).spacing([20.0, 8.0]).show(ui, |ui| {
                for (i, hz) in crate::dsp::EQ_FREQUENCIES.iter().enumerate() {
                    ui.label(if *hz < 1000.0 { format!("{hz:.0} Hz") } else { format!("{:.0} kHz", hz / 1000.0) });
                    ui.add(egui::Slider::new(&mut settings.bands_db[i], -12.0..=12.0).suffix(" dB").fixed_decimals(1));
                    ui.end_row();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Plano").clicked() { settings.bands_db = [0.0; 10]; settings.preamp_db = 0.0; }
                if ui.button("Claridad de voz").clicked() { settings.bands_db = [-6.0, -4.0, -2.0, -1.0, 0.0, 1.0, 3.0, 2.0, 0.0, -2.0]; settings.preamp_db = -3.0; }
                if ui.button("Voz cálida").clicked() { settings.bands_db = [-3.0, 0.0, 3.0, 2.0, 0.0, -1.0, 0.0, 1.0, 0.0, -2.0]; settings.preamp_db = -3.0; }
            });
        });
        NipApp::secondary(ui, "Subir bandas puede saturar: compensá bajando la preamplificación. El ecualizador del micrófono afecta lo que enviás; el de salida sólo lo que escuchás.");
    });
}

#[derive(Clone, Copy, PartialEq)]
enum RoomView {
    Chat,
    Voice,
}
#[derive(Clone, Copy, PartialEq)]
enum SettingsPage {
    Profile,
    Permissions,
    Audio,
    Calls,
    Servers,
    Application,
    Connection,
}

pub fn configure(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = SIDE;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = SELECTED;
    style.visuals.selection.stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, SECONDARY);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(91, 112, 98);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, SECONDARY);
    style.visuals.widgets.active.bg_stroke = Stroke::new(2.0, ACCENT);
    style.spacing.item_spacing = Vec2::new(12.0, 10.0);
    style.spacing.button_padding = Vec2::new(12.0, 8.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

pub struct NipApp {
    commands: UnboundedSender<Command>,
    events: Receiver<Event>,
    state: AppState,
    audio: AudioStatus,
    audio_draft: AudioSettings,
    audio_profile_text: String,
    user: String,
    host_name: String,
    port: String,
    invitation: String,
    endpoint: String,
    invitation_result: Option<String>,
    draft: String,
    channel_drafts: HashMap<String, String>,
    viewed_voice: Option<String>,
    channel_editor: bool,
    channel_target: Option<Channel>,
    channel_name: String,
    channel_kind: ChannelKind,
    delete_channel: Option<Channel>,
    room_view: RoomView,
    error: Option<String>,
    notice: Option<String>,
    settings_open: bool,
    settings_page: SettingsPage,
    name_draft: String,
    profile_loaded: bool,
    host_open: bool,
    host_persistent: bool,
    server_selection: Option<String>,
    server_draft: LocalServerSettings,
    history_pending: bool,
    history_to_start: bool,
    history_last_request: Option<Instant>,
    connect_open: bool,
    invite_open: bool,
    invite_target: Option<String>,
    rename_target: Option<SavedServer>,
    rename_draft: String,
    delete_target: Option<SavedServer>,
    help: Option<egui::Id>,
    help_dismissed: Option<egui::Id>,
    busy: bool,
    test_pending: bool,
    pub quit_at: Option<Instant>,
    pub desktop: Option<crate::desktop::Desktop>,
    notification_pending: bool,
    backup_destination: String,
    backup_archive: String,
    restore_profile: String,
    backup_password: String,
    backup_password_confirmation: String,
    relay_host_json: String,
    relay_member_json: String,
    file_picker: Option<Receiver<NativeSelection>>,
    upload_preview: Option<UploadPreview>,
    convert_for_attachment: Option<String>,
    transfers: Vec<UiTransfer>,
}

#[derive(Clone)]
struct UploadPreview {
    server_id: String,
    channel_id: String,
    destination_label: String,
    path: PathBuf,
    name: String,
    size: u64,
    text: String,
}
enum NativeSelection {
    Upload(Result<Option<UploadPreview>, String>),
    Download(Option<Command>),
}
struct UiTransfer {
    command: Command,
    status: TransferStatus,
}

impl NipApp {
    pub fn new(commands: UnboundedSender<Command>, events: Receiver<Event>) -> Self {
        Self {
            commands,
            events,
            state: AppState::default(),
            audio: AudioStatus::default(),
            audio_draft: AudioSettings::default(),
            audio_profile_text: String::new(),
            user: String::new(),
            host_name: String::new(),
            port: "45454".into(),
            invitation: String::new(),
            endpoint: String::new(),
            invitation_result: None,
            draft: String::new(),
            channel_drafts: HashMap::new(),
            viewed_voice: None,
            channel_editor: false,
            channel_target: None,
            channel_name: String::new(),
            channel_kind: ChannelKind::Text,
            delete_channel: None,
            room_view: RoomView::Chat,
            error: None,
            notice: None,
            settings_open: false,
            settings_page: SettingsPage::Profile,
            name_draft: String::new(),
            profile_loaded: false,
            host_open: false,
            host_persistent: false,
            server_selection: None,
            server_draft: LocalServerSettings::default(),
            history_pending: false,
            history_to_start: false,
            history_last_request: None,
            connect_open: false,
            invite_open: false,
            invite_target: None,
            rename_target: None,
            rename_draft: String::new(),
            delete_target: None,
            help: None,
            help_dismissed: None,
            busy: false,
            test_pending: false,
            quit_at: None,
            desktop: None,
            notification_pending: false,
            backup_destination: String::new(),
            backup_archive: String::new(),
            restore_profile: String::new(),
            backup_password: String::new(),
            backup_password_confirmation: String::new(),
            relay_host_json: String::new(),
            relay_member_json: String::new(),
            file_picker: None,
            upload_preview: None,
            convert_for_attachment: None,
            transfers: Vec::new(),
        }
    }
    fn command(&mut self, command: Command) {
        if self.commands.send(command).is_err() {
            self.error = Some("El servicio se cerró. Cerrá y volvé a abrir NIP.".into());
            self.busy = false;
            self.test_pending = false;
        }
    }
    fn start_picker(
        &mut self,
        ctx: &egui::Context,
        select: impl FnOnce() -> NativeSelection + Send + 'static,
    ) {
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("nip-file-dialog".into())
            .spawn(move || {
                let _ = send.send(select());
                ctx.request_repaint();
            }) {
            Ok(_) => self.file_picker = Some(receive),
            Err(error) => self.error = Some(format!("No se pudo abrir el selector: {error}")),
        }
    }
    fn pick_attachment(&mut self, ctx: &egui::Context, room: &Room) {
        if !room.persistent {
            self.convert_for_attachment = Some(room.id.clone());
            return;
        }
        let server_id = room.id.clone();
        let channel_id = room.selected_text.clone();
        let channel_name = room
            .channels
            .iter()
            .find(|c| c.id == channel_id)
            .map_or("texto", |c| c.name.as_str());
        let destination_label = format!("{} → # {}", room.name, channel_name);
        let text = self.draft.clone();
        self.start_picker(ctx, move || {
            let result = (|| {
                let Some(path) = rfd::FileDialog::new()
                    .set_title("Adjuntar archivo al canal de texto")
                    .pick_file()
                else {
                    return Ok(None);
                };
                let metadata = std::fs::metadata(&path)
                    .map_err(|e| format!("No se pudo revisar el archivo: {e}"))?;
                if !metadata.is_file() {
                    return Err("Elegí un archivo regular".into());
                }
                let name = path
                    .file_name()
                    .ok_or("Nombre de archivo inválido")?
                    .to_string_lossy()
                    .into_owned();
                Ok(Some(UploadPreview {
                    server_id,
                    channel_id,
                    destination_label,
                    path,
                    name,
                    size: metadata.len(),
                    text,
                }))
            })();
            NativeSelection::Upload(result)
        });
    }
    fn operation_id(&mut self) -> Option<String> {
        match (getrandom::u64(), getrandom::u64()) {
            (Ok(a), Ok(b)) => Some(format!("{a:016x}{b:016x}")),
            _ => {
                self.error = Some(
                    "Windows no pudo generar un identificador seguro para la transferencia".into(),
                );
                None
            }
        }
    }
    fn start_transfer(&mut self, command: Command) {
        let (operation_id, name) = match &command {
            Command::UploadFile {
                operation_id, path, ..
            } => (
                operation_id.clone(),
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
            Command::DownloadFile {
                operation_id,
                destination,
                ..
            } => (
                operation_id.clone(),
                destination
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => return,
        };
        self.transfers.push(UiTransfer {
            command: command.clone(),
            status: TransferStatus {
                operation_id,
                name,
                completed: 0,
                total: 0,
                finished: false,
                error: None,
            },
        });
        self.command(command);
    }
    fn transfer_status(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let mut remove = None;
        for (index, transfer) in self.transfers.iter().enumerate() {
            ui.push_id(&transfer.status.operation_id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::Label::new(&transfer.status.name).truncate());
                    if let Some(error) = &transfer.status.error {
                        ui.colored_label(ERROR, error);
                        if ui.button("Reintentar").clicked() {
                            action = Some(transfer.command.clone());
                        }
                    } else if transfer.status.finished {
                        ui.label("Completado");
                    } else {
                        ui.label(format!(
                            "{} / {} bytes",
                            transfer.status.completed, transfer.status.total
                        ));
                        if ui.button("Cancelar transferencia").clicked() {
                            action = Some(Command::CancelTransfer {
                                operation_id: transfer.status.operation_id.clone(),
                            });
                        }
                    }
                    if transfer.status.finished && ui.small_button("Cerrar transferencia").clicked()
                    {
                        remove = Some(index);
                    }
                });
                if !transfer.status.finished && transfer.status.total > 0 {
                    ui.add(egui::ProgressBar::new(
                        (transfer.status.completed as f64 / transfer.status.total as f64)
                            .clamp(0.0, 1.0) as f32,
                    ));
                }
            });
        }
        if let Some(command) = action {
            if let Command::UploadFile { operation_id, .. }
            | Command::DownloadFile { operation_id, .. } = &command
            {
                if let Some(transfer) = self
                    .transfers
                    .iter_mut()
                    .find(|t| &t.status.operation_id == operation_id)
                {
                    transfer.status.finished = false;
                    transfer.status.error = None;
                    transfer.status.completed = 0;
                }
            }
            self.command(command);
        }
        if let Some(index) = remove {
            self.transfers.remove(index);
        }
    }
    fn poll(&mut self) {
        if let Some(result) = self
            .file_picker
            .as_ref()
            .and_then(|picker| picker.try_recv().ok())
        {
            self.file_picker = None;
            match result {
                NativeSelection::Upload(Ok(preview)) => self.upload_preview = preview,
                NativeSelection::Upload(Err(error)) => self.error = Some(error),
                NativeSelection::Download(Some(command)) => self.start_transfer(command),
                NativeSelection::Download(None) => {}
            }
        }
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Transfer(status) => {
                    if let Some(transfer) = self
                        .transfers
                        .iter_mut()
                        .find(|t| t.status.operation_id == status.operation_id)
                    {
                        transfer.status = status;
                    }
                }
                Event::State(state) => {
                    if !self.profile_loaded || state.preferences.port != self.state.preferences.port
                    {
                        self.port = state.preferences.port.to_string();
                    }
                    if !self.profile_loaded || state.display_name != self.state.display_name {
                        self.user = state.display_name.clone();
                        self.name_draft = state.display_name.clone();
                        self.profile_loaded = true;
                    }
                    if self.state.room.as_ref().map(|r| &r.id) != state.room.as_ref().map(|r| &r.id)
                    {
                        self.history_pending = false;
                        self.history_to_start = false;
                        self.history_last_request = None;
                        self.draft.clear();
                        self.channel_drafts.clear();
                        self.viewed_voice = None;
                        self.channel_editor = false;
                        self.delete_channel = None;
                        self.room_view = RoomView::Chat;
                        if state.room.is_some() {
                            self.error = None;
                        }
                    }
                    if let (Some(old), Some(new)) = (&self.state.room, &state.room) {
                        if old.id == new.id && old.selected_text != new.selected_text {
                            self.channel_drafts
                                .insert(old.selected_text.clone(), std::mem::take(&mut self.draft));
                            self.draft = self
                                .channel_drafts
                                .remove(&new.selected_text)
                                .unwrap_or_default();
                            self.history_pending = false;
                            self.history_to_start = false;
                            self.history_last_request = None;
                        }
                        self.channel_drafts
                            .retain(|id, _| new.channels.iter().any(|c| &c.id == id));
                    }
                    self.busy = state.connecting;
                    if let Some(id) = &self.server_selection {
                        let old = self
                            .state
                            .saved
                            .iter()
                            .find(|s| &s.id == id)
                            .and_then(|s| s.local.as_ref());
                        let new = state
                            .saved
                            .iter()
                            .find(|s| &s.id == id)
                            .and_then(|s| s.local.as_ref());
                        if old != new {
                            if let Some(settings) = new {
                                self.server_draft = settings.clone();
                            }
                        }
                    }
                    self.state = state;
                }
                Event::History { messages, has_more } => {
                    if let Some(room) = &mut self.state.room {
                        room.history.extend(messages);
                        room.history.sort_by_key(|m| m.id);
                        room.history.dedup_by_key(|m| m.id);
                        room.has_older_messages = has_more;
                    }
                    self.history_pending = false;
                    self.history_to_start = true;
                }
                Event::Members(members) => {
                    if self.state.preferences.join_leave_sounds && !self.audio.deafened {
                        if let Some(room) = &self.state.room {
                            let joined = members.iter().any(|m| {
                                m.calling
                                    && !room.members.iter().any(|old| old.id == m.id && old.calling)
                            });
                            let left = room.members.iter().any(|old| {
                                old.calling && !members.iter().any(|m| m.id == old.id && m.calling)
                            });
                            if joined || left {
                                self.command(Command::PlayCue { joined });
                            }
                        }
                    }
                    if let Some(room) = &mut self.state.room {
                        room.members = members;
                    }
                }
                Event::Chat(message) => {
                    if self.state.preferences.notifications
                        && self
                            .state
                            .room
                            .as_ref()
                            .is_some_and(|r| r.self_id != message.sender_id)
                    {
                        self.notification_pending = true;
                    }
                    if let Some(room) = &mut self.state.room {
                        if message.channel_id == room.selected_text
                            && !room.history.iter().any(|m| m.id == message.id)
                        {
                            room.history.push(message);
                            room.history.sort_by_key(|m| m.id);
                            if !room.persistent && room.history.len() > 100 {
                                room.history.remove(0);
                            }
                        }
                    }
                }
                Event::Audio(audio) => {
                    if audio.error.is_none()
                        && self
                            .audio
                            .error
                            .as_ref()
                            .is_some_and(|old| self.error.as_ref() == Some(old))
                    {
                        self.error = None;
                    }
                    self.audio_draft = audio.settings.clone();
                    self.audio = audio;
                    self.test_pending = false;
                }
                Event::Notice(notice) => {
                    self.notice = Some(notice);
                }
                Event::Invitation(invitation) => {
                    self.invitation_result = Some(invitation);
                    self.busy = false;
                }
                Event::Error(error) => {
                    self.history_pending = false;
                    self.error = Some(error);
                    self.busy = false;
                    self.test_pending = false;
                }
                Event::Disconnected(reason) => {
                    self.state.room = None;
                    self.draft.clear();
                    self.channel_drafts.clear();
                    self.busy = false;
                    self.error = Some(reason);
                }
            }
        }
    }
    fn send_chat(&mut self) {
        let text = self.draft.trim();
        if let Some(room) = self
            .state
            .room
            .as_ref()
            .filter(|_| !text.is_empty() && text.chars().count() <= 2000)
        {
            let command = Command::SendChat {
                server_id: room.id.clone(),
                channel_id: room.selected_text.clone(),
                text: text.into(),
            };
            self.command(command);
            self.draft.clear();
        }
    }
    fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
        ui.add(egui::Button::new(RichText::new(label).color(BG)).fill(ACCENT))
    }
    fn secondary(ui: &mut egui::Ui, text: impl Into<String>) {
        ui.label(RichText::new(text).color(SECONDARY));
    }
    fn render(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let ctx = ui.ctx().clone();
        if let Some(desktop) = &self.desktop {
            if desktop.quitting() {
                self.command(Command::Shutdown);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.state.preferences.minimize_to_tray
                && !self.quit_at.is_some_and(|at| Instant::now() >= at)
                && ctx.input(|i| i.viewport().close_requested())
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        if self.notification_pending {
            self.notification_pending = false;
            if !ctx.input(|i| i.focused) {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
            }
        }
        if self
            .quit_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.command(Command::Shutdown);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // A quiet idle window repaints only to receive background state, never to capture audio.
        ctx.request_repaint_after(Duration::from_millis(
            if self.audio.testing || self.audio.calling {
                100
            } else {
                200
            },
        ));
        if self.settings_open {
            self.render_settings(ui);
            return;
        }
        egui::Panel::left("servers")
            .exact_size(180.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("NIP").size(27.0).strong());
                    if icon_button(ui, Icon::Settings, "Abrir ajustes", false).clicked() {
                        self.open_settings(SettingsPage::Profile);
                    }
                });
                ui.add_space(12.0);
                Self::secondary(ui, "Tu nombre");
                ui.add(
                    egui::Label::new(if self.user.is_empty() {
                        "Sin configurar"
                    } else {
                        &self.user
                    })
                    .wrap(),
                );
                if ui.small_button("Editar perfil").clicked() {
                    self.open_settings(SettingsPage::Profile);
                }
                ui.add_space(16.0);
                let mut saved = self.state.saved.clone();
                if let Some(room) = &self.state.room {
                    if !saved.iter().any(|s| s.id == room.id) {
                        saved.push(SavedServer {
                            id: room.id.clone(),
                            name: room.name.clone(),
                            endpoint: String::new(),
                            alias: None,
                            owned: self.state.hosts.iter().any(|h| h.id == room.id),
                            local: None,
                        });
                    }
                }
                ui.label(RichText::new(format!("Servidores · {}", saved.len())).strong());
                if saved.is_empty() {
                    Self::secondary(ui, "Los servidores que conectes aparecerán acá.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("saved_servers")
                    .max_height((ui.available_height() - 180.0).max(70.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for server in saved {
                            self.server_row(ui, &server);
                        }
                    });
                ui.add_space(12.0);
                if ui.button("Agregar servidor").clicked() {
                    self.connect_open = true;
                }
                if ui.button("Crear servidor").clicked() {
                    self.host_open = true;
                }
                if self.busy {
                    Self::secondary(ui, "Conectando…");
                    if ui.button("Cancelar conexión").clicked() {
                        self.command(Command::CancelConnect);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    Self::secondary(
                        ui,
                        if self.state.hosts.len() == 1 {
                            "1 servidor propio en línea".into()
                        } else {
                            format!("{} servidores propios en línea", self.state.hosts.len())
                        },
                    );
                    if let Some(host) = self.state.host.clone() {
                        if ui.button("Invitar personas").clicked() {
                            self.invite_target = Some(host.id.clone());
                            self.invite_open = true;
                            self.invitation_result = None;
                            self.endpoint = suggested_endpoint(&host.addresses);
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Anfitrión · {}", host.name))
                                    .color(SECONDARY),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&host.name);
                    }
                });
            });
        egui::Panel::left("channels")
            .exact_size(208.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDE).inner_margin(16))
            .show(ui, |ui| self.channels(ui));
        egui::Panel::bottom("voice_controls")
            .frame(egui::Frame::new().fill(SIDE).inner_margin(16))
            .show(ui, |ui| self.voice_bar(ui));
        if !self.transfers.is_empty() {
            egui::Panel::bottom("file_transfers")
                .frame(egui::Frame::new().fill(BG).inner_margin(8))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("transfer_status")
                        .max_height(100.0)
                        .show(ui, |ui| self.transfer_status(ui));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(20))
            .show(ui, |ui| {
                if let Some(error) = self.error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("Atención: {error}")).color(ERROR));
                        if ui.small_button("Cerrar aviso").clicked() {
                            self.error = None;
                        }
                    });
                    ui.add_space(12.0);
                }
                if let Some(room) = self.state.room.clone() {
                    self.room(ui, room);
                } else {
                    ui.add_space(56.0);
                    ui.label(
                        RichText::new("Una sala para estar cerca.")
                            .size(28.0)
                            .strong(),
                    );
                    ui.add_space(12.0);
                    Self::secondary(
                        ui,
                        "Creá un servidor o conectate con una invitación privada.",
                    );
                    Self::secondary(ui, "Podés chatear sin activar el micrófono.");
                    ui.add_space(24.0);
                    if Self::primary(ui, "Conectar con invitación").clicked() {
                        self.connect_open = true;
                    }
                }
            });
        self.dialogs(&ctx);
    }
    fn server_row(&mut self, ui: &mut egui::Ui, server: &SavedServer) {
        ui.push_id(&server.id, |ui| {
            let width = ui.available_width();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let selected = self.state.room.as_ref().is_some_and(|r| r.id == server.id);
                let response = ui
                    .add_enabled_ui(!self.busy && !self.user.trim().is_empty(), |ui| {
                        ui.add_sized(
                            [(width - 36.0).max(40.0), 40.0],
                            egui::Button::new(
                                RichText::new(server.display_name()).color(if selected {
                                    ACCENT
                                } else {
                                    TEXT
                                }),
                            )
                            .selected(selected)
                            .truncate(),
                        )
                    })
                    .inner;
                if response
                    .on_hover_text(format!("{} · {}", server.display_name(), server.endpoint))
                    .clicked()
                    && !selected
                {
                    self.busy = true;
                    self.command(Command::Reconnect {
                        id: server.id.clone(),
                        user: self.user.trim().into(),
                    });
                }
                let menu = ui.add_sized([28.0, 40.0], egui::Button::new(""));
                menu.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("Opciones de {}", server.display_name()),
                    )
                });
                for offset in [-5.0, 0.0, 5.0] {
                    ui.painter().circle_filled(
                        menu.rect.center() + egui::vec2(offset, 0.0),
                        1.5,
                        TEXT,
                    );
                }
                egui::Popup::menu(&menu).show(|ui| {
                    ui.set_min_width(190.0);
                    if ui.button("Cambiar nombre").clicked() {
                        self.rename_draft = server.display_name().into();
                        self.rename_target = Some(server.clone());
                        ui.close();
                    }
                    if server.owned && ui.button("Configurar servidor").clicked() {
                        self.server_selection = Some(server.id.clone());
                        self.server_draft = server.local.clone().unwrap_or_default();
                        self.open_settings(SettingsPage::Servers);
                        ui.close();
                    }
                    let hosted = self.state.hosts.iter().any(|h| h.id == server.id);
                    if server.owned && !hosted {
                        if ui
                            .add_enabled(
                                !self.busy && !self.user.trim().is_empty(),
                                egui::Button::new("Encender servidor"),
                            )
                            .clicked()
                        {
                            self.busy = true;
                            self.command(Command::StartServer {
                                id: server.id.clone(),
                                user: self.user.trim().into(),
                            });
                            ui.close();
                        }
                    }
                    if (hosted || !server.owned)
                        && ui
                            .add_enabled(hosted, egui::Button::new("Apagar servidor"))
                            .on_disabled_hover_text(
                                "Sólo podés apagar un servidor que está alojado en este equipo.",
                            )
                            .clicked()
                    {
                        self.command(Command::StopServer {
                            id: server.id.clone(),
                        });
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(RichText::new("Borrar servidor").color(ERROR))
                        .clicked()
                    {
                        self.delete_target = Some(server.clone());
                        ui.close();
                    }
                });
            });
        });
    }
    fn channels(&mut self, ui: &mut egui::Ui) {
        let Some(room) = self.state.room.clone() else {
            ui.heading("Canales");
            Self::secondary(ui, "Elegí un servidor para ver su chat y su sala de voz.");
            ui.add_space(20.0);
            if ui.button("Revisar mi audio").clicked() {
                self.open_settings(SettingsPage::Audio);
            }
            return;
        };
        ui.add(egui::Label::new(RichText::new(&room.name).size(20.0).strong()).wrap());
        Self::secondary(
            ui,
            format!(
                "Conectado · {} {}",
                room.members.len(),
                if room.members.len() == 1 {
                    "persona"
                } else {
                    "personas"
                }
            ),
        );
        let admin = self.state.saved.iter().any(|s| s.id == room.id && s.owned);
        if admin && ui.button("Crear canal").clicked() {
            self.channel_target = None;
            self.channel_name.clear();
            self.channel_kind = ChannelKind::Text;
            self.channel_editor = true;
        }
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("channel_list")
            .show(ui, |ui| {
                for kind in [ChannelKind::Text, ChannelKind::Voice] {
                    ui.label(
                        RichText::new(if kind == ChannelKind::Text {
                            "Canales de texto"
                        } else {
                            "Salas de voz"
                        })
                        .strong(),
                    );
                    for channel in room.channels.iter().filter(|c| c.kind == kind) {
                        let connected = room
                            .members
                            .iter()
                            .find(|m| m.id == room.self_id)
                            .and_then(|m| m.voice_channel.as_deref())
                            == Some(channel.id.as_str());
                        ui.push_id(&channel.id, |ui| {
                            ui.horizontal(|ui| {
                                let label = if kind == ChannelKind::Text {
                                    format!("# {}", channel.name)
                                } else {
                                    format!(
                                        "{} · {}",
                                        channel.name,
                                        if connected { "conectado" } else { "voz" }
                                    )
                                };
                                let selected = if kind == ChannelKind::Text {
                                    self.room_view == RoomView::Chat
                                        && room.selected_text == channel.id
                                } else {
                                    connected
                                };
                                let width = (ui.available_width() - if admin { 40.0 } else { 0.0 })
                                    .max(48.0);
                                let response = ui.add_sized(
                                    [width, 36.0],
                                    egui::Button::new(RichText::new(&label))
                                        .truncate()
                                        .selected(selected),
                                );
                                response.clone().on_hover_text(&label);
                                if response.clicked() {
                                    if kind == ChannelKind::Text {
                                        self.room_view = RoomView::Chat;
                                        if room.selected_text != channel.id {
                                            self.command(Command::SelectText {
                                                channel_id: channel.id.clone(),
                                            });
                                        }
                                    } else {
                                        self.room_view = RoomView::Voice;
                                        self.viewed_voice = Some(channel.id.clone());
                                        if !connected {
                                            self.command(Command::JoinVoice {
                                                channel_id: channel.id.clone(),
                                            });
                                        }
                                    }
                                }
                                if admin {
                                    response.context_menu(|ui| {
                                        self.channel_actions(ui, &room, channel)
                                    });
                                    let menu = ui.add_sized([28.0, 36.0], egui::Button::new(""));
                                    let label = format!("Opciones del canal {}", channel.name);
                                    menu.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                                    });
                                    for offset in [-5.0, 0.0, 5.0] {
                                        ui.painter().circle_filled(
                                            menu.rect.center() + egui::vec2(offset, 0.0),
                                            1.5,
                                            TEXT,
                                        );
                                    }
                                    egui::Popup::menu(&menu)
                                        .show(|ui| self.channel_actions(ui, &room, channel));
                                    menu.on_hover_text(label);
                                }
                            });
                            if kind == ChannelKind::Voice {
                                ui.indent("participants", |ui| {
                                    for member in room.members.iter().filter(|m| {
                                        m.voice_channel.as_deref() == Some(channel.id.as_str())
                                    }) {
                                        ui.add(
                                            egui::Label::new(format!(
                                                "{}{}",
                                                member.name,
                                                if member.muted { " · silenciado" } else { "" }
                                            ))
                                            .truncate(),
                                        );
                                    }
                                });
                            }
                        });
                    }
                    ui.add_space(12.0);
                }
                Self::secondary(
                    ui,
                    "Un clic en voz entra a esa sala. Elegir texto conserva la llamada.",
                );
            });
    }
    fn channel_actions(&mut self, ui: &mut egui::Ui, room: &Room, channel: &Channel) {
        if ui.button("Cambiar nombre").clicked() {
            self.channel_name = channel.name.clone();
            self.channel_kind = channel.kind;
            self.channel_target = Some(channel.clone());
            self.channel_editor = true;
            ui.close();
        }
        let can_delete = channel.kind != ChannelKind::Text
            || room
                .channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Text)
                .count()
                > 1;
        if ui
            .add_enabled(can_delete, egui::Button::new("Borrar canal"))
            .on_disabled_hover_text("El servidor debe conservar al menos un canal de texto.")
            .clicked()
        {
            self.delete_channel = Some(channel.clone());
            ui.close();
        }
    }
    fn voice_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let status = if self.audio.testing {
                "Prueba privada · sin envío"
            } else if self.audio.calling && self.audio.deafened {
                "Ensordecido"
            } else if self.audio.calling && !self.audio.ready {
                "Iniciando audio…"
            } else if self.audio.calling && self.audio.active_input.is_empty() && self.audio.error.is_some() {
                "Modo escucha · entrada no disponible"
            } else if self.audio.calling && self.audio.settings.advanced.push_to_talk && !self.audio.muted {
                if self.audio.push_to_talk_active { "Hablando · tecla pulsada" } else { "Pulsá tu atajo para hablar" }
            } else if self.audio.calling {
                if self.audio.deafened {
                    "Ensordecido"
                } else if self.audio.muted {
                    "Micrófono silenciado"
                } else {
                    "En llamada"
                }
            } else {
                "Fuera de llamada"
            };
            ui.label(
                RichText::new(status).color(if self.audio.calling || self.audio.testing {
                    ACCENT
                } else {
                    SECONDARY
                }),
            );
            if self.audio.calling {
                if ui.button("Salir de llamada").clicked() {
                    self.command(Command::Leave);
                }
            } else if ui
                .add_enabled(
                    self.state.room.is_some() && !self.audio.testing && !self.test_pending,
                    egui::Button::new("Entrar a llamada"),
                )
                .clicked()
            {
                self.command(Command::Join);
            }
            if ui.add_enabled_ui(!self.audio.testing && !self.audio.deafened && !self.test_pending, |ui| {
                icon_button(ui, Icon::Mic, if self.audio.muted { "Activar micrófono" } else { "Silenciar micrófono" }, self.audio.muted)
            }).inner.on_disabled_hover_text(if self.audio.deafened {
                "Desactivá Ensordecer para cambiar el micrófono."
            } else {
                "Detené la prueba privada para cambiar el micrófono. Tu voz no se envía al grupo."
            })
            .clicked()
            {
                self.command(Command::Mute);
            }
            if icon_button(
                ui,
                Icon::Headphones,
                if self.audio.deafened {
                    "Volver a escuchar"
                } else {
                    "Ensordecer"
                },
                self.audio.deafened,
            )
            .clicked()
            {
                self.command(Command::Deafen);
            }
            if icon_button(ui, Icon::Settings, "Ajustes de audio", self.settings_open).clicked() {
                self.open_settings(SettingsPage::Audio);
            }
        });
    }
    fn room(&mut self, ui: &mut egui::Ui, room: Room) {
        let voice_id_owned = self.viewed_voice.clone().or_else(|| {
            room.members
                .iter()
                .find(|m| m.id == room.self_id)
                .and_then(|m| m.voice_channel.clone())
        });
        let voice_id = voice_id_owned.as_deref();
        let shown = if self.room_view == RoomView::Chat {
            room.channels.iter().find(|c| c.id == room.selected_text)
        } else {
            room.channels
                .iter()
                .find(|c| Some(c.id.as_str()) == voice_id)
        };
        let title = shown.map_or_else(
            || "Canales".into(),
            |c| {
                if c.kind == ChannelKind::Text {
                    format!("# {}", c.name)
                } else {
                    format!("{} · voz", c.name)
                }
            },
        );
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(title).size(24.0).strong()).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Desconectar").clicked() {
                    self.command(Command::Disconnect);
                }
            });
        });
        if self.room_view == RoomView::Voice {
            ui.add_space(16.0);
            Self::secondary(ui, "Conversá por voz con las personas de este servidor.");
            Self::secondary(
                ui,
                "Podés abrir cualquier canal de texto sin salir de la llamada.",
            );
            ui.add_space(20.0);
            for member in room
                .members
                .iter()
                .filter(|m| m.voice_channel.as_deref() == voice_id && voice_id.is_some())
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&member.name).strong());
                    if member.id != room.self_id {
                        self.participant_audio_menu(ui, member);
                    }
                    Self::secondary(
                        ui,
                        if member.muted {
                            "Micrófono apagado"
                        } else {
                            "En llamada"
                        },
                    );
                });
            }
            if !room
                .members
                .iter()
                .any(|m| m.voice_channel.as_deref() == voice_id && voice_id.is_some())
            {
                Self::secondary(ui, "La sala está vacía. Podés ser el primero en entrar.");
            }
            ui.add_space(20.0);
            if !self.audio.calling
                && ui
                    .add_enabled(
                        !self.audio.testing && !self.test_pending,
                        egui::Button::new("Entrar a llamada"),
                    )
                    .clicked()
            {
                if let Some(channel_id) = voice_id {
                    self.command(Command::JoinVoice {
                        channel_id: channel_id.into(),
                    });
                }
            }
            return;
        }
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("text_members")
            .max_height(64.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for member in &room.members {
                        let status = if member.calling {
                            if member.muted {
                                "micrófono silenciado"
                            } else {
                                "en llamada"
                            }
                        } else {
                            "Fuera de llamada"
                        };
                        ui.label(RichText::new(&member.name).strong())
                            .on_hover_text(status);
                        Self::secondary(ui, status);
                        if member.id != room.self_id {
                            self.participant_audio_menu(ui, member);
                        }
                    }
                });
            });
        ui.add_space(20.0);
        if room.persistent {
            Self::secondary(ui, "Historial guardado en el equipo del administrador");
            if room.has_older_messages
                && ui
                    .add_enabled(
                        !self.history_pending
                            && self
                                .history_last_request
                                .is_none_or(|last| last.elapsed() >= Duration::from_secs(1)),
                        egui::Button::new(if self.history_pending {
                            "Cargando mensajes…"
                        } else {
                            "Cargar mensajes anteriores"
                        }),
                    )
                    .clicked()
            {
                self.history_pending = true;
                self.history_last_request = Some(Instant::now());
                self.command(Command::History {
                    before: room.history.first().map_or(u64::MAX, |m| m.id),
                });
            }
        }
        let composer_height = 152.0;
        let mut history_scroll = egui::ScrollArea::vertical()
            .id_salt("chat_history")
            .stick_to_bottom(true)
            .max_height((ui.available_height() - composer_height).max(0.0))
            .auto_shrink([false, false]);
        if std::mem::take(&mut self.history_to_start) {
            history_scroll = history_scroll.vertical_scroll_offset(0.0);
        }
        history_scroll.show(ui, |ui| {
            if room.history.is_empty() {
                Self::secondary(ui, "La conversación empieza acá. Escribí para saludar.");
            }
            for message in &room.history {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&message.sender_name).strong());
                    let seconds = message.sent_at % 86400;
                    Self::secondary(
                        ui,
                        format!("{:02}:{:02} UTC", seconds / 3600, seconds / 60 % 60),
                    );
                });
                ui.add(egui::Label::new(&message.text).wrap());
                for attachment in &message.attachments {
                    ui.add(egui::Label::new(RichText::new(&attachment.name).strong()).truncate())
                        .on_hover_text(&attachment.name);
                    Self::secondary(
                        ui,
                        format!(
                            "Archivo · {} bytes · original del anfitrión",
                            attachment.size
                        ),
                    );
                    if ui
                        .add_enabled(
                            self.file_picker.is_none(),
                            egui::Button::new("Descargar archivo…"),
                        )
                        .clicked()
                    {
                        if let Some(operation_id) = self.operation_id() {
                            let server_id = room.id.clone();
                            let channel_id = message.channel_id.clone();
                            let attachment_id = attachment.id.clone();
                            let name = attachment.name.clone();
                            self.start_picker(ui.ctx(), move || {
                                let destination = rfd::FileDialog::new()
                                    .set_title("Guardar una copia nueva del adjunto")
                                    .set_file_name(name)
                                    .save_file();
                                NativeSelection::Download(destination.map(|destination| {
                                    Command::DownloadFile {
                                        server_id,
                                        channel_id,
                                        attachment_id,
                                        destination,
                                        operation_id,
                                    }
                                }))
                            });
                        }
                    }
                }
                ui.add_space(12.0);
            }
        });
        ui.add_space(12.0);
        if ui
            .add_enabled(
                self.file_picker.is_none() && self.upload_preview.is_none(),
                egui::Button::new(if self.file_picker.is_some() {
                    "Selector de archivo abierto…"
                } else {
                    "Adjuntar archivo…"
                }),
            )
            .clicked()
        {
            self.pick_attachment(ui.ctx(), &room);
        }
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.draft)
                    .desired_rows(2)
                    .return_key(egui::KeyboardShortcut::new(
                        egui::Modifiers::SHIFT,
                        egui::Key::Enter,
                    ))
                    .id(egui::Id::unique("chat_composer"))
                    .hint_text("Mensaje al grupo")
                    .char_limit(2000)
                    .desired_width((ui.available_width() - 90.0).max(80.0)),
            );
            let enter = response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let send = ui
                .add_enabled(
                    !self.draft.trim().is_empty() && self.draft.chars().count() <= 2000,
                    egui::Button::new("Enviar"),
                )
                .clicked();
            if enter || send {
                self.send_chat();
                response.request_focus();
            }
        });
        Self::secondary(
            ui,
            format!(
                "{} / 2000 · Enter envía · Shift+Enter nueva línea",
                self.draft.chars().count()
            ),
        );
    }

    fn participant_audio_menu(&mut self, ui: &mut egui::Ui, member: &Member) {
        ui.push_id(member.id, |ui| {
            ui.menu_button("Volumen", |ui| {
                ui.set_max_width(300.0);
                ui.label(format!("Audio de {} · sólo para vos", member.name));
                let mut preference = self.audio.participants.iter().find(|p| p.id == member.id).cloned().unwrap_or(ParticipantAudio { id: member.id, volume: 1.0, muted: false });
                let before = preference.clone();
                ui.checkbox(&mut preference.muted, "Silenciar localmente");
                let mut percent = preference.volume * 100.0;
                if ui.add(egui::Slider::new(&mut percent, 0.0..=200.0).suffix(" %")).changed() { preference.volume = percent / 100.0; }
                Self::secondary(ui, "Se mantiene durante esta conexión. No cambia lo que escuchan las otras personas.");
                if before.volume != preference.volume || before.muted != preference.muted { self.command(Command::ParticipantAudio(preference)); }
            });
        });
    }
    fn help_button(
        &mut self,
        ui: &mut egui::Ui,
        title: &'static str,
        text: &'static str,
    ) -> egui::Response {
        let response = ui.push_id(title, |ui| ui.small_button("i")).inner;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Ayuda: {title}"))
        });
        let id = response.id;
        if !response.hovered() && !response.has_focus() && self.help_dismissed == Some(id) {
            self.help_dismissed = None;
        }
        if response.clicked() {
            if self.help == Some(id) {
                self.help = None;
                self.help_dismissed = Some(id);
            } else {
                self.help = Some(id);
                self.help_dismissed = None;
            }
        }
        let mut open = self.help == Some(id)
            || (self.help_dismissed != Some(id) && (response.hovered() || response.has_focus()));
        let was_open = open;
        egui::Popup::from_response(&response)
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_max_width(320.0);
                ui.label(text);
            });
        if was_open && !open {
            self.help = None;
            self.help_dismissed = Some(id);
        }
        response
    }
    fn open_settings(&mut self, page: SettingsPage) {
        if self.settings_page != page {
            self.clear_sensitive_drafts();
            self.help = None;
            self.help_dismissed = None;
        }
        if self.settings_page == SettingsPage::Audio && page != SettingsPage::Audio {
            self.stop_private_test();
        }
        self.settings_page = page;
        self.settings_open = true;
        if page == SettingsPage::Audio {
            self.command(Command::RefreshDevices);
        }
    }
    fn stop_private_test(&mut self) {
        if self.audio.testing || self.test_pending {
            self.command(Command::StopTest);
        }
    }
    fn render_settings(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.clear_sensitive_drafts();
            self.stop_private_test();
            self.settings_open = false;
        }
        egui::Panel::top("settings_header")
            .frame(egui::Frame::new().fill(BG).inner_margin(20))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Ajustes");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cerrar ajustes").clicked() {
                            self.clear_sensitive_drafts();
                            self.stop_private_test();
                            self.settings_open = false;
                        }
                        Self::secondary(ui, "Esc para volver");
                    });
                });
                if let Some(error) = self
                    .error
                    .clone()
                    .filter(|error| self.audio.error.as_ref() != Some(error))
                {
                    ui.add_space(8.0);
                    ui.colored_label(ERROR, error);
                    if ui.small_button("Cerrar aviso").clicked() {
                        self.error = None;
                    }
                }
            });
        egui::Panel::left("settings_navigation")
            .exact_size(184.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDE).inner_margin(16))
            .show(ui, |ui| {
                for (page, label) in [
                    (SettingsPage::Profile, "Perfil"),
                    (SettingsPage::Permissions, "Permisos"),
                    (SettingsPage::Audio, "Voz y audio"),
                    (SettingsPage::Calls, "Llamadas"),
                    (SettingsPage::Servers, "Servidores"),
                    (SettingsPage::Application, "Aplicación"),
                    (SettingsPage::Connection, "Conexión"),
                ] {
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.0],
                            egui::Button::new(label).selected(self.settings_page == page),
                        )
                        .clicked()
                    {
                        self.open_settings(page);
                    }
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(24))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("settings_content", self.settings_page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(640.0));
                        if let Some(notice) = &self.notice {
                            ui.colored_label(ACCENT, notice);
                            if ui.small_button("Cerrar aviso").clicked() {
                                self.notice = None;
                            }
                            ui.add_space(12.0);
                        }
                        match self.settings_page {
                            SettingsPage::Profile => self.profile_settings(ui),
                            SettingsPage::Permissions => self.permissions_settings(ui),
                            SettingsPage::Audio => self.settings(ui),
                            SettingsPage::Calls => self.call_settings(ui),
                            SettingsPage::Servers => self.server_settings(ui),
                            SettingsPage::Application => self.application_settings(ui),
                            SettingsPage::Connection => self.connection_settings(ui),
                        }
                    });
            });
    }
    fn application_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Aplicación");
        Self::secondary(ui, "Inicio, segundo plano y avisos.");
        ui.add_space(24.0);
        let mut p = self.state.preferences.clone();
        ui.checkbox(&mut p.start_with_windows, "Iniciar con Windows");
        Self::secondary(
            ui,
            "Inicia esta copia de NIP con este perfil al entrar a Windows. No activa el micrófono ni inicia servidores.",
        );
        ui.add_enabled_ui(self.desktop.is_some(), |ui| {
            ui.checkbox(
                &mut p.minimize_to_tray,
                "Al cerrar, minimizar al área de notificación",
            );
        });
        Self::secondary(
            ui,
            "Usá Abrir NIP o Salir desde el icono de la bandeja. Mientras NIP sigue abierto, el servidor y las llamadas continúan.",
        );
        ui.checkbox(
            &mut p.notifications,
            "Avisar de mensajes nuevos en la barra de tareas",
        );
        Self::secondary(
            ui,
            "Solicita atención si NIP está en segundo plano. No muestra el contenido de mensajes en Windows.",
        );
        ui.add_space(24.0);
        ui.label(RichText::new("Atajos globales").strong());
        for (label, id, value) in [
            (
                "Silenciar / activar micrófono",
                "mute_shortcut",
                &mut p.mute_shortcut,
            ),
            (
                "Ensordecer / volver a escuchar",
                "deafen_shortcut",
                &mut p.deafen_shortcut,
            ),
        ] {
            ui.label(label);
            egui::ComboBox::from_id_salt(id)
                .selected_text(if *value == 0 {
                    "Desactivado".into()
                } else {
                    hotkey_label(*value)
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(value, 0, "Desactivado");
                    for key in (0x70..=0x7B).chain([0x05, 0x06]) {
                        ui.selectable_value(value, key, hotkey_label(key));
                    }
                });
        }
        Self::secondary(
            ui,
            "Funcionan también en segundo plano. Evitá usar la misma tecla que Pulsar para hablar o que tus juegos.",
        );
        if p != self.state.preferences {
            self.command(Command::Preferences(p));
        }
    }
    fn connection_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Conexión");
        Self::secondary(ui, "Diagnóstico local y alcance del servidor.");
        ui.add_space(24.0);
        if ui.button("Comprobar red local y conexión actual").clicked() {
            self.command(Command::DiagnoseNetwork);
        }
        let diagnostics = &self.state.diagnostics;
        if let Some(endpoint) = &diagnostics.remote_endpoint {
            ui.label(format!("Conectado a {endpoint}"));
            if let Some(transport) = &diagnostics.transport {
                ui.label(transport);
            }
        } else {
            ui.label("Sin conexión activa");
        }
        if let Some(endpoint) = &diagnostics.pending_endpoint {
            ui.label(format!("Intentando conectar a {endpoint}"));
        }
        if let Some(error) = &diagnostics.last_error {
            ui.colored_label(ERROR, format!("Último intento: {error}"));
        }
        if !diagnostics.udp_status.is_empty() {
            ui.add_space(16.0);
            ui.label(&diagnostics.udp_status);
            for address in &diagnostics.local_addresses {
                ui.label(address);
            }
            if let Some(rtt) = diagnostics.rtt_ms {
                ui.label(format!(
                    "Ida y vuelta QUIC: {rtt:.1} ms · {} paquetes enviados · {} perdidos",
                    diagnostics.sent_packets, diagnostics.lost_packets
                ));
            }
            Self::secondary(
                ui,
                "Resultados de la última comprobación. Poder enlazar un puerto local no demuestra acceso desde Internet.",
            );
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Acceso desde otra casa · sin verificar").strong());
        Self::secondary(
            ui,
            "El motor usa QUIC sobre UDP. No abre automáticamente el firewall ni el router. Sin una comprobación desde otra red no se puede confirmar el acceso externo ni detectar CGNAT.",
        );
        ui.add_space(16.0);
        ui.label(RichText::new("Relay autoalojable · configuración por servidor").strong());
        Self::secondary(
            ui,
            "Configurá los JSON de anfitrión y miembro en Ajustes → Servidores con el servidor apagado. Cliente y anfitrión abren túneles TLS de salida. No hay un servicio público preconfigurado ni WAN validada.",
        );
        ui.add_space(16.0);
        if ui.button("Configurar puerto del servidor").clicked() {
            self.open_settings(SettingsPage::Servers);
        }
    }
    fn call_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Llamadas");
        Self::secondary(ui, "Comportamiento de conexión y privacidad.");
        ui.add_space(24.0);
        let mut preferences = self.state.preferences.clone();
        ui.checkbox(
            &mut preferences.join_leave_sounds,
            "Sonidos cuando alguien entra o sale de la llamada",
        );
        ui.checkbox(
            &mut preferences.auto_reconnect,
            "Reconectar si se pierde la conexión",
        );
        if preferences != self.state.preferences {
            self.command(Command::Preferences(preferences));
        }
        Self::secondary(
            ui,
            "Realiza hasta 5 intentos con espera creciente. Vuelve al servidor sin entrar a la llamada ni activar el micrófono. Desconectar o cancelar detiene los intentos.",
        );
        ui.add_space(16.0);
        let mut settings = self.audio_draft.clone();
        if ui
            .checkbox(
                &mut settings.advanced.auto_device,
                "Seguir cambios de dispositivos y usar el predeterminado si falla el elegido",
            )
            .changed()
        {
            self.command(Command::AudioSettings(settings.clone()));
        }
        Self::secondary(
            ui,
            "Comprueba cambios cada 2 segundos. Si Windows no puede abrir el dispositivo elegido, intenta la entrada y salida predeterminadas sin borrar tu selección guardada.",
        );
        if ui
            .checkbox(
                &mut settings.advanced.join_muted,
                "Entrar con el micrófono silenciado",
            )
            .changed()
        {
            self.command(Command::AudioSettings(settings));
        }
        if ui
            .button("Configurar micrófono, atajos y procesamiento")
            .clicked()
        {
            self.open_settings(SettingsPage::Audio);
        }
    }
    fn server_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Servidores");
        Self::secondary(
            ui,
            "Administrá los servidores guardados en tu equipo. Aplicá los cambios con Guardar configuración del servidor.",
        );
        ui.add_space(24.0);
        let owned: Vec<_> = self
            .state
            .saved
            .iter()
            .filter(|s| s.owned)
            .cloned()
            .collect();
        if let Some(server) = self
            .server_selection
            .as_ref()
            .and_then(|id| owned.iter().find(|s| &s.id == id))
            .or_else(|| owned.first())
            .cloned()
        {
            if self.server_selection.as_deref() != Some(&server.id) {
                self.server_selection = Some(server.id.clone());
                self.server_draft = server.local.clone().unwrap_or_default();
            }
            egui::ComboBox::from_id_salt("server_settings_selection")
                .width(ui.available_width())
                .truncate()
                .selected_text(server.display_name())
                .show_ui(ui, |ui| {
                    for choice in &owned {
                        if ui
                            .selectable_label(choice.id == server.id, choice.display_name())
                            .clicked()
                        {
                            self.clear_sensitive_drafts();
                            self.server_selection = Some(choice.id.clone());
                            self.server_draft = choice.local.clone().unwrap_or_default();
                        }
                    }
                });
            let hosted = self.state.hosts.iter().any(|h| h.id == server.id);
            Self::secondary(
                ui,
                if hosted {
                    "En línea en este equipo"
                } else {
                    "Apagado · sus datos guardados se conservan"
                },
            );
            ui.add_space(16.0);
            ui.label(RichText::new("Tipo de servidor").strong());
            let was_persistent = self.server_draft.persistent;
            ui.radio_value(
                &mut self.server_draft.persistent,
                false,
                "De sesión · chat temporal",
            );
            ui.radio_value(
                &mut self.server_draft.persistent,
                true,
                "Persistente · datos en este equipo",
            );
            if !was_persistent && self.server_draft.persistent {
                self.server_draft.auto_start = true;
            }
            if !self.server_draft.persistent {
                self.server_draft.auto_start = false;
            }
            Self::secondary(
                ui,
                if self.server_draft.persistent {
                    "Guarda el historial en este equipo. Apagar o cerrar NIP no lo borra. Los demás pueden entrar mientras NIP esté abierto y el servidor sea accesible por red."
                } else {
                    "Los mensajes nuevos son temporales. Si antes fue persistente, su historial guardado se conserva en disco y vuelve a estar disponible al convertirlo nuevamente."
                },
            );
            ui.add_enabled_ui(self.server_draft.persistent, |ui| {
                ui.checkbox(
                    &mut self.server_draft.auto_start,
                    "Iniciar este servidor al abrir NIP",
                );
            });
            Self::secondary(
                ui,
                "Iniciar el servidor no te conecta a su llamada ni activa el micrófono. Para mantenerlo disponible en segundo plano podés minimizar NIP a la bandeja.",
            );
            ui.add_space(16.0);
            ui.label("Puerto UDP de este servidor");
            ui.add(egui::DragValue::new(&mut self.server_draft.port).range(1..=65535));
            let port_changed_live = hosted
                && server
                    .local
                    .as_ref()
                    .is_some_and(|old| old.port != self.server_draft.port);
            if port_changed_live {
                ui.colored_label(ERROR, "Apagá este servidor antes de cambiar su puerto.");
            }
            ui.label("Participantes máximos · incluye al anfitrión");
            ui.add(egui::Slider::new(
                &mut self.server_draft.max_participants,
                1..=32,
            ));
            ui.add_space(16.0);
            ui.label(RichText::new("Almacenamiento de adjuntos").strong());
            if let Some(room) = self.state.room.as_ref().filter(|r| r.id == server.id) {
                Self::secondary(
                    ui,
                    format!(
                        "Uso confirmado: {} bytes · Reservado por cargas activas: {} bytes",
                        room.storage_usage.used_bytes, room.storage_usage.reserved_bytes
                    ),
                );
            } else {
                Self::secondary(
                    ui,
                    "Conectate a este servidor para consultar su uso actual.",
                );
            }
            let mut file_mb = self.server_draft.file_limit_bytes as f64 / 1_000_000.0;
            ui.label("Máximo por archivo · MB decimales");
            if ui
                .add(
                    egui::DragValue::new(&mut file_mb)
                        .range(0.001..=1_000_000.0)
                        .speed(1.0)
                        .suffix(" MB"),
                )
                .changed()
            {
                self.server_draft.file_limit_bytes = (file_mb * 1_000_000.0).round() as u64;
            }
            let mut server_gb = self.server_draft.storage_limit_bytes as f64 / 1_000_000_000.0;
            ui.label("Cuota del servidor · GB decimales");
            if ui
                .add(
                    egui::DragValue::new(&mut server_gb)
                        .range(0.001..=1_000_000.0)
                        .speed(0.1)
                        .suffix(" GB"),
                )
                .changed()
            {
                self.server_draft.storage_limit_bytes =
                    (server_gb * 1_000_000_000.0).round() as u64;
            }
            Self::secondary(
                ui,
                "1 MB = 1.000.000 bytes; 1 GB = 1.000.000.000 bytes. Reducir la cuota por debajo del uso bloquea cargas nuevas y conserva los archivos existentes. No hay borrado automático. Los adjuntos durables requieren servidor persistente.",
            );
            if ui
                .add_enabled(
                    !port_changed_live && server.local.as_ref() != Some(&self.server_draft),
                    egui::Button::new("Guardar configuración del servidor"),
                )
                .clicked()
            {
                self.command(Command::ConfigureServer {
                    id: server.id.clone(),
                    settings: self.server_draft.clone(),
                });
            }
            ui.add_space(16.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(if hosted {
                        "Apagar servidor"
                    } else {
                        "Encender y entrar"
                    })
                    .clicked()
                {
                    self.command(if hosted {
                        Command::StopServer {
                            id: server.id.clone(),
                        }
                    } else {
                        Command::StartServer {
                            id: server.id.clone(),
                            user: self.user.trim().into(),
                        }
                    });
                }
                if ui.button("Regenerar invitaciones").clicked() {
                    self.command(Command::RegenerateInvitation {
                        id: server.id.clone(),
                    });
                }
            });
            Self::secondary(
                ui,
                "Regenerar revoca las invitaciones anteriores. Quienes ya están conectados pueden permanecer. Borrar servidor, en su menú de tres puntos, elimina también el historial local.",
            );
            ui.add_space(16.0);
            egui::CollapsingHeader::new("Relay autoalojable · configuración avanzada").show(ui, |ui| {
                Self::secondary(ui, "El relay debe estar disponible y configurado por su operador. Aplicá estos datos con el servidor apagado. La credencial del anfitrión es privada; no la compartas con miembros.");
                ui.label("Contenido de relay-host.json · secreto del anfitrión");
                ui.add(egui::TextEdit::multiline(&mut self.relay_host_json).password(true).char_limit(32768).desired_rows(2).desired_width(ui.available_width()));
                ui.label("Contenido de relay-member.json · configuración para miembros");
                ui.add(egui::TextEdit::multiline(&mut self.relay_member_json).password(true).char_limit(32768).desired_rows(2).desired_width(ui.available_width()));
                let ready = !hosted && !self.relay_host_json.trim().is_empty() && !self.relay_member_json.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("Aplicar configuración de relay")).clicked() {
                    let parsed = (serde_json::from_str::<crate::relay::RelayEndpoint>(&self.relay_host_json), serde_json::from_str::<crate::relay::RelayEndpoint>(&self.relay_member_json));
                    match parsed {
                        (Ok(host), Ok(client)) => self.command(Command::ConfigureRelay { server_id: server.id.clone(), configuration: Some(RelayConfiguration { host, client, tunnels: 4 }) }),
                        _ => self.error = Some("Configuración de relay inválida. Pegá el contenido completo de los dos archivos generados por el operador.".into()),
                    }
                    drop(age::secrecy::SecretString::from(std::mem::take(&mut self.relay_host_json)));
                    drop(age::secrecy::SecretString::from(std::mem::take(&mut self.relay_member_json)));
                }
                if ui.add_enabled(!hosted, egui::Button::new("Desactivar relay de este servidor")).clicked() {
                    self.command(Command::ConfigureRelay { server_id: server.id.clone(), configuration: None });
                }
                Self::secondary(ui, "Conexión muestra el transporte realmente usado. Aplicar un relay no valida acceso WAN ni activa el micrófono.");
            });
        } else {
            Self::secondary(
                ui,
                "Creá un servidor para configurar su modalidad, inicio e historial local.",
            );
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        egui::CollapsingHeader::new("Valores por defecto para servidores nuevos").show(ui, |ui| {
            let mut preferences = self.state.preferences.clone();
            ui.label("Puerto UDP propuesto"); ui.add(egui::DragValue::new(&mut preferences.port).range(1..=65535));
            ui.label("Participantes máximos propuestos"); ui.add(egui::Slider::new(&mut preferences.max_participants, 1..=32));
            Self::secondary(ui, "Cada servidor guarda su configuración. Dos servidores activos deben usar puertos distintos. Cambiar estos valores no modifica los ya creados ni configura el router.");
            if preferences != self.state.preferences { self.command(Command::Preferences(preferences)); }
        });
    }
    fn profile_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Perfil");
        Self::secondary(ui, "Tu identidad en los servidores de NIP.");
        ui.add_space(24.0);
        ui.label(RichText::new("Nombre por defecto").strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.name_draft)
                .hint_text("Cómo te ven tus amigos")
                .char_limit(64)
                .desired_width(ui.available_width()),
        );
        Self::secondary(
            ui,
            "Se guarda en este equipo y se usa en las próximas conexiones. Para actualizar el nombre en la sala actual, reconectate.",
        );
        ui.add_space(16.0);
        let valid =
            !self.name_draft.trim().is_empty() && self.name_draft.trim().chars().count() <= 64;
        if ui
            .add_enabled(
                valid && self.name_draft.trim() != self.state.display_name,
                egui::Button::new("Guardar nombre"),
            )
            .clicked()
        {
            self.command(Command::SetDisplayName(self.name_draft.trim().into()));
        }
        if !self.state.display_name.is_empty() && self.name_draft.trim() == self.state.display_name
        {
            ui.colored_label(ACCENT, "Nombre guardado en este equipo.");
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        self.backup_settings(ui);
    }
    fn clear_sensitive_drafts(&mut self) {
        drop(age::secrecy::SecretString::from(std::mem::take(
            &mut self.backup_password,
        )));
        drop(age::secrecy::SecretString::from(std::mem::take(
            &mut self.backup_password_confirmation,
        )));
        drop(age::secrecy::SecretString::from(std::mem::take(
            &mut self.relay_host_json,
        )));
        drop(age::secrecy::SecretString::from(std::mem::take(
            &mut self.relay_member_json,
        )));
    }
    fn backup_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Respaldo cifrado del perfil");
        Self::secondary(
            ui,
            "Incluye identidad, invitaciones, ajustes, historial y adjuntos confirmados. La contraseña protege la copia portable; guardala en un lugar seguro. Sin ella no se puede restaurar.",
        );
        ui.label("Contraseña del respaldo");
        ui.add(
            egui::TextEdit::singleline(&mut self.backup_password)
                .password(true)
                .desired_width(ui.available_width()),
        );
        egui::CollapsingHeader::new("Exportar copia portable").show(ui, |ui| {
            ui.label("Archivo de destino nuevo · ruta completa");
            ui.add(egui::TextEdit::singleline(&mut self.backup_destination).hint_text("C:\\Respaldos\\NIP.age").desired_width(ui.available_width()));
            ui.label("Repetir contraseña");
            ui.add(egui::TextEdit::singleline(&mut self.backup_password_confirmation).password(true).desired_width(ui.available_width()));
            Self::secondary(ui, "Usá una contraseña larga y única. La exportación no reemplaza archivos existentes.");
            let valid = !self.backup_destination.trim().is_empty() && !self.backup_password.is_empty()
                && self.backup_password == self.backup_password_confirmation;
            if ui.add_enabled(valid, egui::Button::new("Exportar respaldo cifrado")).clicked() {
                let password = age::secrecy::SecretString::from(std::mem::take(&mut self.backup_password));
                drop(age::secrecy::SecretString::from(std::mem::take(&mut self.backup_password_confirmation)));
                self.command(Command::ExportBackup { destination: self.backup_destination.trim().into(), password });
            }
        });
        egui::CollapsingHeader::new("Restaurar en un perfil aislado").show(ui, |ui| {
            ui.label("Archivo de respaldo · ruta completa");
            ui.add(egui::TextEdit::singleline(&mut self.backup_archive).desired_width(ui.available_width()));
            ui.label("Carpeta nueva para el perfil restaurado · ruta completa");
            ui.add(egui::TextEdit::singleline(&mut self.restore_profile).desired_width(ui.available_width()));
            Self::secondary(ui, "La carpeta de destino no debe existir. El perfil actual se conserva. La copia se restaura sin servidores ni reconexión automáticos; revisá el inventario antes de iniciarla explícitamente.");
            let valid = !self.backup_archive.trim().is_empty() && !self.restore_profile.trim().is_empty() && !self.backup_password.is_empty();
            if ui.add_enabled(valid, egui::Button::new("Restaurar copia sin iniciar servidores")).clicked() {
                let password = age::secrecy::SecretString::from(std::mem::take(&mut self.backup_password));
                drop(age::secrecy::SecretString::from(std::mem::take(&mut self.backup_password_confirmation)));
                self.command(Command::RestoreBackup { archive: self.backup_archive.trim().into(), profile: self.restore_profile.trim().into(), password });
            }
        });
    }
    fn permissions_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Permisos");
        Self::secondary(
            ui,
            "Windows administra el acceso al micrófono y el firewall. Revisá los permisos concedidos en sus ajustes.",
        );
        ui.add_space(24.0);
        ui.label(RichText::new("Micrófono").strong());
        Self::secondary(
            ui,
            if self.audio.ready && self.audio.captured_frames > 0 {
                "Acceso comprobado en la captura actual."
            } else {
                "Sin comprobar en esta sesión. Revisá el permiso en Windows o iniciá una prueba de micrófono."
            },
        );
        Self::secondary(
            ui,
            "Sólo se captura al entrar a una llamada o iniciar una prueba privada. No se guardan grabaciones.",
        );
        if ui
            .button("Revisar permisos de micrófono en Windows")
            .clicked()
        {
            self.command(Command::OpenMicrophonePermissions);
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Red y firewall").strong());
        Self::secondary(
            ui,
            "Abrir un servidor en NIP no comprueba que el firewall permita conexiones ni abre el puerto del router.",
        );
        if ui.button("Abrir Seguridad de Windows").clicked() {
            self.command(Command::OpenSecuritySettings);
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Acceso a servidores").strong());
        Self::secondary(
            ui,
            format!(
                "{} invitaciones guardadas en este perfil, protegidas con el cifrado del usuario de Windows.",
                self.state.saved.len()
            ),
        );
        Self::secondary(
            ui,
            "Una invitación permite entrar al servidor. No concede acceso al micrófono; ese permiso es independiente.",
        );
        for server in &self.state.saved {
            ui.label(server.display_name());
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Voz y audio");
        Self::secondary(
            ui,
            "Elegí tus dispositivos y comprobá el micrófono antes de entrar a una llamada.",
        );
        ui.add_space(24.0);
        if let Some(error) = &self.audio.error {
            ui.colored_label(ERROR, format!("Audio: {error}"));
            ui.add_space(16.0);
        }
        let before = self.audio_draft.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Micrófono de entrada").strong());
            self.help_button(ui, "Entrada", "Cambiar el micrófono detiene la prueba privada. No modifica el dispositivo predeterminado de Windows.");
        });
        egui::ComboBox::from_id_salt("input_device")
            .width(ui.available_width())
            .truncate()
            .selected_text(if self.audio_draft.input.is_empty() {
                "Predeterminada de Windows"
            } else {
                &self.audio_draft.input
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.audio_draft.input,
                    String::new(),
                    "Predeterminada de Windows",
                );
                for device in &self.audio.inputs {
                    ui.selectable_value(&mut self.audio_draft.input, device.clone(), device);
                }
            });
        if !self.audio_draft.input.is_empty()
            && !self.audio.inputs.contains(&self.audio_draft.input)
        {
            ui.colored_label(
                ERROR,
                "La entrada guardada no está disponible. Elegí otro micrófono.",
            );
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Salida de audio").strong());
            self.help_button(ui, "Salida", "Elegí dónde escuchar al grupo y la prueba privada. Usá auriculares para evitar acople.");
        });
        egui::ComboBox::from_id_salt("output_device")
            .width(ui.available_width())
            .truncate()
            .selected_text(if self.audio_draft.output.is_empty() {
                "Predeterminada de Windows"
            } else {
                &self.audio_draft.output
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.audio_draft.output,
                    String::new(),
                    "Predeterminada de Windows",
                );
                for device in &self.audio.outputs {
                    ui.selectable_value(&mut self.audio_draft.output, device.clone(), device);
                }
            });
        if !self.audio_draft.output.is_empty()
            && !self.audio.outputs.contains(&self.audio_draft.output)
        {
            ui.colored_label(
                ERROR,
                "La salida guardada no está disponible. Elegí otra salida.",
            );
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.audio.sound_testing && !self.audio.deafened,
                    egui::Button::new(if self.audio.sound_testing {
                        "Reproduciendo prueba…"
                    } else {
                        "Probar salida · 2 segundos"
                    }),
                )
                .clicked()
            {
                self.command(Command::TestSound);
            }
            Self::secondary(
                ui,
                format!(
                    "{} entradas y {} salidas detectadas",
                    self.audio.inputs.len(),
                    self.audio.outputs.len()
                ),
            );
            if ui.small_button("Actualizar dispositivos").clicked() {
                self.command(Command::RefreshDevices);
            }
        });
        if self.audio.ready && !self.audio.active_input.is_empty() {
            Self::secondary(
                ui,
                format!(
                    "Entrada activa: {}\nSalida activa: {}",
                    self.audio.active_input, self.audio.active_output
                ),
            );
        }
        if self.audio_draft != before {
            self.command(Command::AudioSettings(self.audio_draft.clone()));
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Prueba de micrófono").strong());
        Self::secondary(
            ui,
            "Usá auriculares: vas a escuchar tu voz. Durante la prueba no se envía audio al grupo.",
        );
        ui.add_space(12.0);
        if ui
            .add_enabled(
                !self.test_pending && (!self.audio.deafened || self.audio.testing),
                egui::Button::new(if self.test_pending {
                    "Abriendo audio…"
                } else if self.audio.testing {
                    "Detener prueba"
                } else {
                    "Probar micrófono"
                }),
            )
            .clicked()
        {
            self.test_pending = true;
            self.command(if self.audio.testing {
                Command::StopTest
            } else {
                Command::StartTest
            });
        }
        Self::secondary(
            ui,
            if self.audio.testing {
                "Nivel de entrada · prueba activa"
            } else if self.audio.calling && !self.audio.deafened {
                "Nivel de entrada · en llamada"
            } else {
                "Nivel de entrada · prueba detenida"
            },
        );
        let raw = if self.audio.ready {
            self.audio.raw_level
        } else {
            0.0
        };
        let db = if raw > 0.0 && raw.is_finite() {
            format!("{:.0} dBFS", 20.0 * raw.log10())
        } else {
            "sin señal".into()
        };
        ui.label(format!("Original · antes de filtros ({db})"));
        ui.add(
            egui::ProgressBar::new(meter_level(raw))
                .fill(if meter_level(raw) > 0.0 { ACCENT } else { BG })
                .desired_height(12.0)
                .desired_width(ui.available_width()),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::ProgressIndicator,
                true,
                "Nivel original del micrófono",
            )
        });
        ui.label("Procesada · filtros y nivel, antes del umbral");
        ui.add(
            egui::ProgressBar::new(meter_level(self.audio.level))
                .fill(if meter_level(self.audio.level) > 0.0 {
                    ACCENT
                } else {
                    BG
                })
                .desired_height(12.0)
                .desired_width(ui.available_width()),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::ProgressIndicator,
                true,
                "Nivel de entrada del micrófono",
            )
        });
        if self.audio.ready && self.audio.input_clipping {
            ui.colored_label(ERROR, "La entrada está saturando. Bajá el nivel del micrófono en Windows o alejá el micrófono de la boca; los filtros no recuperan una señal ya recortada.");
        }
        ui.add_space(8.0);
        if self.audio.deafened {
            Self::secondary(ui, "Desactivá Ensordecer para probar el micrófono.");
        }
        if self.audio.testing && !self.audio.ready {
            Self::secondary(ui, "Abriendo el micrófono y la salida seleccionados…");
        }
        if self.audio.testing && self.audio.ready {
            ui.colored_label(ACCENT, "Escucha local activa · sin envío al grupo");
            if self.audio.captured_frames == 0 {
                Self::secondary(
                    ui,
                    "El dispositivo abrió; esperando muestras del micrófono.",
                );
            } else if self.audio.level < 0.001 {
                Self::secondary(
                    ui,
                    if self.audio.raw_level > 0.001 {
                        "Hay señal original, pero la procesada es muy baja. Revisá nivel, ecualización y supresión de ruido; el perfil Bajo consumo permite comparar."
                    } else {
                        "El dispositivo entrega muestras, pero la entrada es muy baja. Revisá el mute físico y el micrófono seleccionado, y hablá cerca de él."
                    },
                );
            } else {
                Self::secondary(ui, "Señal de micrófono detectada.");
            }
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Niveles").strong());
        ui.add_space(8.0);
        let before_levels = self.audio_draft.clone();
        let slider_width = (ui.available_width() - 105.0).max(140.0);
        ui.spacing_mut().slider_width = slider_width;
        ui.label("Nivel del micrófono");
        let mut level = self.audio_draft.input_level * 100.0;
        if ui
            .add(egui::Slider::new(&mut level, 0.0..=100.0).suffix(" %"))
            .changed()
        {
            self.audio_draft.input_level = level / 100.0;
        }
        ui.add_space(12.0);
        ui.label("Volumen de la app");
        let mut volume = self.audio_draft.volume * 100.0;
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=100.0).suffix(" %"))
            .changed()
        {
            self.audio_draft.volume = volume / 100.0;
        }
        Self::secondary(
            ui,
            "Estos niveles sólo afectan a NIP, no al volumen global de Windows.",
        );
        if self.audio_draft != before_levels {
            self.command(Command::AudioSettings(self.audio_draft.clone()));
        }
        self.advanced_audio_settings(ui);
        ui.add_space(20.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Abrir Sonido de Windows").clicked() {
                self.command(Command::OpenSoundSettings);
            }
            if ui.button("Restablecer audio").clicked() {
                self.command(Command::AudioSettings(AudioSettings::default()));
            }
        });
    }
    fn advanced_audio_settings(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().slider_width = (ui.available_width() - 128.0).max(140.0);
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(16.0);
        ui.label(RichText::new("Opciones avanzadas").strong());
        Self::secondary(
            ui,
            "Se guardan en este equipo. La prueba privada permite escuchar los cambios del micrófono.",
        );
        let before = self.audio_draft.clone();
        let a = &mut self.audio_draft.advanced;
        ui.label("Perfil de procesamiento");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Bajo consumo").clicked() {
                a.noise_suppression = 0;
                a.echo_cancellation = false;
                a.automatic_gain = false;
                a.processing_48khz = false;
                a.opus_complexity = 2;
                a.bitrate = 16000;
            }
            if ui.button("Voz equilibrada").clicked() {
                a.noise_suppression = 2;
                a.echo_cancellation = true;
                a.automatic_gain = true;
                a.processing_48khz = false;
                a.opus_complexity = 5;
                a.bitrate = 32000;
            }
            if ui.button("Mayor calidad").clicked() {
                a.noise_suppression = 2;
                a.echo_cancellation = true;
                a.automatic_gain = true;
                a.processing_48khz = true;
                a.opus_complexity = 10;
                a.bitrate = 64000;
            }
        });
        Self::secondary(
            ui,
            "Los perfiles cambian procesamiento, complejidad de Opus y bitrate. Podés ajustar cada valor abajo. Una supresión más fuerte puede deformar la voz.",
        );
        ui.add_space(12.0);
        egui::CollapsingHeader::new("Micrófono · avanzado").id_salt("advanced_microphone").show(ui, |ui| {
            ui.checkbox(&mut a.join_muted, "Entrar a las llamadas con el micrófono silenciado");
            ui.checkbox(&mut a.push_to_talk, "Pulsar para hablar");
            ui.add_enabled_ui(a.push_to_talk, |ui| {
                egui::ComboBox::from_id_salt("ptt_key").selected_text(hotkey_label(a.push_to_talk_key)).show_ui(ui, |ui| {
                    for key in (0x70..=0x7B).chain([0x05, 0x06]) { ui.selectable_value(&mut a.push_to_talk_key, key, hotkey_label(key)); }
                });
            });
            Self::secondary(ui, "El atajo funciona con NIP en segundo plano. Elegí una tecla que no use tu juego; no se envía voz mientras esté silenciado o ensordecido.");
            ui.add_space(12.0);
            ui.label("Supresión de ruido · WebRTC");
            egui::ComboBox::from_id_salt("noise_suppression").selected_text(["Desactivada", "Baja · 6 dB", "Moderada · 12 dB", "Alta · 18 dB", "Muy alta · 21 dB"][a.noise_suppression.min(4) as usize]).show_ui(ui, |ui| {
                for (i, label) in ["Desactivada", "Baja · 6 dB", "Moderada · 12 dB", "Alta · 18 dB", "Muy alta · 21 dB"].iter().enumerate() { ui.selectable_value(&mut a.noise_suppression, i as u8, *label); }
            });
            ui.checkbox(&mut a.echo_cancellation, "Cancelación de eco · AEC3");
            Self::secondary(ui, "Usa el audio reproducido por NIP como referencia. No cancela sonidos reproducidos por otras aplicaciones. Con auriculares puede dejarse desactivada.");
            ui.add_enabled_ui(a.echo_cancellation, |ui| {
                ui.label("Retardo estimado de entrada y salida");
                ui.add(egui::Slider::new(&mut a.echo_delay_ms, 0..=500).suffix(" ms"));
            });
            ui.checkbox(&mut a.automatic_gain, "Ganancia automática · AGC2 digital");
            Self::secondary(ui, "Ajusta tu voz dentro de NIP sin cambiar el nivel del micrófono en Windows.");
            ui.add_enabled_ui(a.automatic_gain, |ui| {
                ui.label("Margen antes de saturación");
                ui.add(egui::Slider::new(&mut a.gain_headroom_db, 0.0..=20.0).suffix(" dB"));
                ui.label("Amplificación automática máxima");
                ui.add(egui::Slider::new(&mut a.gain_max_db, 0.0..=50.0).suffix(" dB"));
            });
            ui.checkbox(&mut a.processing_48khz, "Procesamiento interno a 48 kHz");
            Self::secondary(ui, "Desactivado: hasta 32 kHz internos para ahorrar recursos. El transporte sigue siendo mono a 48 kHz con bloques de 20 ms.");
            ui.add_space(16.0);
            ui.checkbox(&mut a.voice_activation, "Activación por nivel de voz");
            Self::secondary(ui, "Transmite cuando el nivel supera el umbral. No distingue una voz del teclado ni sustituye la supresión de ruido.");
            ui.add_enabled_ui(a.voice_activation, |ui| {
                ui.label("Umbral de activación");
                ui.add(egui::Slider::new(&mut a.threshold_db, -72.0..=-12.0).suffix(" dBFS").fixed_decimals(0));
                Self::secondary(ui, "Más cerca de −72: detecta sonidos bajos. Más cerca de −12: exige una voz más fuerte.");
                ui.label("Pausa antes de cerrar el micrófono");
                ui.add(egui::Slider::new(&mut a.release_ms, 40..=1000).step_by(20.0).suffix(" ms"));
                Self::secondary(ui, "Evita cortar las pausas entre palabras.");
            });
            ui.add_space(12.0);
            ui.checkbox(&mut a.high_pass, "Filtrar graves del micrófono (80 Hz)");
            Self::secondary(ui, "Reduce vibraciones y zumbidos graves. No es cancelación de eco ni filtrado de voces de fondo.");
            if a.voice_activation && self.audio.ready {
                ui.colored_label(if self.audio.gate_open { ACCENT } else { SECONDARY }, if self.audio.gate_open { "Umbral superado · entrada abierta" } else { "Por debajo del umbral · entrada cerrada" });
            }
        });
        egui::CollapsingHeader::new("Audio · avanzado").id_salt("advanced_playback").show(ui, |ui| {
            ui.label("Volumen de la prueba privada");
            let mut monitor = a.monitor_volume * 100.0;
            if ui.add(egui::Slider::new(&mut monitor, 0.0..=100.0).suffix(" %").fixed_decimals(0)).changed() { a.monitor_volume = monitor / 100.0; }
            Self::secondary(ui, "Sólo cambia cuánto te escuchás en la prueba. No modifica el volumen de otras personas ni tu envío al grupo.");
            ui.add_space(12.0);
            ui.checkbox(&mut a.mix_limiter, "Evitar saturación al mezclar voces");
            Self::secondary(ui, "Reduce el nivel de los bloques que exceden el máximo cuando hablan varias personas. No cambia el volumen de otras apps.");
            ui.add_space(12.0);
            ui.label("Calidad de voz enviada");
            egui::ComboBox::from_id_salt("voice_bitrate").width(ui.available_width()).selected_text(match a.bitrate {
                16000 => "Ahorro · 16 kb/s", 32000 => "Equilibrada · 32 kb/s", 64000 => "Alta · 64 kb/s", _ => "Automática · Opus",
            }).show_ui(ui, |ui| {
                for (value, label) in [(0, "Automática · Opus"), (16000, "Ahorro · 16 kb/s"), (32000, "Equilibrada · 32 kb/s"), (64000, "Alta · 64 kb/s")] { ui.selectable_value(&mut a.bitrate, value, label); }
            });
            Self::secondary(ui, "Una calidad mayor usa más ancho de banda. Este ajuste no cambia la frecuencia del dispositivo ni resuelve problemas de conexión.");
            ui.label("Complejidad de codificación Opus");
            ui.add(egui::Slider::new(&mut a.opus_complexity, 0..=10));
            Self::secondary(ui, "0 usa menos CPU; 10 dedica más cálculo a codificar. No cambia el volumen.");
            ui.checkbox(&mut a.opus_vbr, "Bitrate variable (VBR)");
            ui.checkbox(&mut a.opus_dtx, "Ahorrar ancho de banda durante silencios (DTX)");
            ui.checkbox(&mut a.opus_fec, "Enviar redundancia para pérdida de paquetes (FEC)");
            Self::secondary(ui, "La redundancia aumenta el tráfico. El receptor intenta recuperar una pérdida con el siguiente paquete; si no hay redundancia disponible, Opus disimula el hueco. No evita cortes bajo pérdidas prolongadas.");
            ui.label("Pérdida de paquetes prevista para el codificador");
            ui.add(egui::Slider::new(&mut a.packet_loss_percent, 0..=100).suffix(" %"));
        });
        equalizer_settings(
            ui,
            "Ecualizador del micrófono",
            "mic_eq",
            &mut a.microphone_eq,
        );
        equalizer_settings(ui, "Ecualizador de salida", "out_eq", &mut a.playback_eq);
        Self::secondary(
            ui,
            format!(
                "Diagnóstico de audio: {} discontinuidades recuperables · procesamiento de entrada: {:.2} ms por bloque de 20 ms. No representa el consumo total de CPU.",
                self.audio.buffer_interruptions,
                self.audio.processing_us as f64 / 1000.0
            ),
        );
        if ui.button("Restablecer opciones avanzadas").clicked() {
            self.audio_draft.advanced = AdvancedAudioSettings::default();
        }
        if self.audio_draft != before {
            self.command(Command::AudioSettings(self.audio_draft.clone()));
        }
        egui::CollapsingHeader::new("Perfiles de audio · importar y exportar JSON").id_salt("audio_json_profiles").show(ui, |ui| {
            Self::secondary(ui, "Compartí tus ajustes de procesamiento, dispositivos y Opus. El archivo contiene preferencias de audio; las credenciales del servidor se guardan por separado.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Exportar ajustes actuales").clicked() {
                    match serde_json::to_string_pretty(&self.audio_draft) { Ok(text) => self.audio_profile_text = text, Err(error) => self.error = Some(format!("No se pudo exportar: {error}")) }
                }
                if ui.add_enabled(!self.audio_profile_text.is_empty(), egui::Button::new("Copiar JSON")).clicked() { ui.ctx().copy_text(self.audio_profile_text.clone()); }
            });
            ui.add(egui::TextEdit::multiline(&mut self.audio_profile_text).char_limit(32768).desired_rows(8).desired_width(ui.available_width()).hint_text("Pegá un perfil JSON para importarlo"));
            if ui.add_enabled(!self.audio_profile_text.trim().is_empty(), egui::Button::new("Aplicar perfil JSON")).clicked() {
                match serde_json::from_str::<AudioSettings>(&self.audio_profile_text) {
                    Ok(settings) => self.command(Command::AudioSettings(settings)),
                    Err(error) => self.error = Some(format!("Perfil de audio inválido: {error}")),
                }
            }
        });
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut preview) = self.upload_preview.take() {
            let mut send = false;
            let mut cancel = false;
            let connected = self.state.room.as_ref().is_some_and(|r| {
                r.id == preview.server_id
                    && r.persistent
                    && r.channels.iter().any(|c| c.id == preview.channel_id)
            });
            let response = egui::Modal::new(egui::Id::unique("upload_preview")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Revisar adjunto");
                ui.label(&preview.destination_label);
                ui.add(egui::Label::new(&preview.name).wrap());
                ui.label(format!("{} bytes · {:.3} MB", preview.size, preview.size as f64 / 1_000_000.0));
                ui.label("Mensaje opcional");
                ui.add(egui::TextEdit::multiline(&mut preview.text).char_limit(2000).desired_rows(2).desired_width(ui.available_width()));
                Self::secondary(ui, "El archivo se guarda en el anfitrión antes de publicar el mensaje. Se podrá descargar mientras el servidor esté en línea; no se abrirá automáticamente.");
                if !connected { ui.colored_label(ERROR, "Volvé a conectar al servidor de destino antes de enviar."); }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Cancelar adjunto").clicked() { cancel = true; }
                    if ui.add_enabled(connected, egui::Button::new("Enviar archivo y mensaje")).clicked() { send = true; }
                });
            });
            if send {
                if let Some(operation_id) = self.operation_id() {
                    if self.state.room.as_ref().is_some_and(|r| {
                        r.id == preview.server_id && r.selected_text == preview.channel_id
                    }) && self.draft == preview.text
                    {
                        self.draft.clear();
                    }
                    self.start_transfer(Command::UploadFile {
                        server_id: preview.server_id,
                        channel_id: preview.channel_id,
                        path: preview.path,
                        operation_id,
                        text: preview.text,
                    });
                }
            } else if !cancel && !response.should_close() {
                self.upload_preview = Some(preview);
            }
        }
        if let Some(server_id) = self.convert_for_attachment.clone() {
            if let Some(room) = self.state.room.clone().filter(|room| room.id == server_id) {
                let local = self
                    .state
                    .saved
                    .iter()
                    .find(|s| s.id == room.id && s.owned)
                    .and_then(|s| s.local.clone());
                let response = egui::Modal::new(egui::Id::unique("attachment_persistence")).show(ctx, |ui| {
                    ui.set_width(360.0); ui.heading("El adjunto necesita un servidor persistente");
                    Self::secondary(ui, "Los servidores de sesión no aceptan archivos durables. Convertir conserva los mensajes todavía disponibles y guarda los nuevos en el equipo del anfitrión. No recupera mensajes temporales anteriores.");
                    if let Some(mut settings) = local {
                        if ui.button("Convertir este servidor a persistente").clicked() {
                            settings.persistent = true;
                            self.command(Command::ConfigureServer { id: room.id, settings });
                            self.convert_for_attachment = None;
                        }
                    } else { Self::secondary(ui, "Pedile al administrador que convierta este servidor antes de adjuntar."); }
                    if ui.button("Volver sin adjuntar").clicked() { self.convert_for_attachment = None; }
                });
                if response.should_close() {
                    self.convert_for_attachment = None;
                }
            } else {
                self.convert_for_attachment = None;
            }
        }
        if self.channel_editor {
            if let Some(room) = self.state.room.clone() {
                let response =
                    egui::Modal::new(egui::Id::unique("channel_editor")).show(ctx, |ui| {
                        ui.set_width(360.0);
                        ui.heading(if self.channel_target.is_some() {
                            "Cambiar nombre del canal"
                        } else {
                            "Crear canal"
                        });
                        if self.channel_target.is_none() {
                            ui.radio_value(
                                &mut self.channel_kind,
                                ChannelKind::Text,
                                "Texto · mensajes y archivos",
                            );
                            ui.radio_value(
                                &mut self.channel_kind,
                                ChannelKind::Voice,
                                "Voz · llamada por sala",
                            );
                        }
                        ui.label("Nombre del canal");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.channel_name)
                                .char_limit(64)
                                .desired_width(ui.available_width()),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Cancelar").clicked() {
                                self.channel_editor = false;
                            }
                            if ui
                                .add_enabled(
                                    !self.channel_name.trim().is_empty(),
                                    egui::Button::new("Guardar canal"),
                                )
                                .clicked()
                            {
                                let name = self.channel_name.trim().into();
                                if let Some(target) = &self.channel_target {
                                    self.command(Command::RenameChannel {
                                        server_id: room.id.clone(),
                                        channel_id: target.id.clone(),
                                        name,
                                    });
                                } else {
                                    self.command(Command::CreateChannel {
                                        server_id: room.id.clone(),
                                        name,
                                        kind: self.channel_kind,
                                    });
                                }
                                self.channel_editor = false;
                            }
                        });
                    });
                if response.should_close() {
                    self.channel_editor = false;
                }
            }
        }
        if let (Some(channel), Some(room)) = (self.delete_channel.clone(), self.state.room.clone())
        {
            let response = egui::Modal::new(egui::Id::unique("delete_channel")).show(ctx, |ui| {
                ui.set_width(390.0);
                ui.heading("Borrar canal");
                ui.label(format!("¿Borrar «{}»?", channel.name));
                Self::secondary(ui, "Se borrarán permanentemente el historial y los adjuntos de este canal. Los otros canales, las copias descargadas y los respaldos anteriores se conservan. No se puede deshacer; hacé un backup antes.");
                ui.horizontal(|ui| {
                    if ui.button("Cancelar").clicked() { self.delete_channel = None; }
                    if ui.button(RichText::new("Borrar canal y contenido").color(ERROR)).clicked() {
                        self.command(Command::DeleteChannel { server_id: room.id, channel_id: channel.id });
                        self.delete_channel = None;
                    }
                });
            });
            if response.should_close() {
                self.delete_channel = None;
            }
        }
        if let Some(server) = self.rename_target.clone() {
            let response = egui::Modal::new(egui::Id::unique("rename_server")).show(ctx, |ui| {
                ui.set_width(390.0); ui.heading("Cambiar nombre");
                Self::secondary(ui, if server.owned { "El nuevo nombre se muestra a las personas conectadas al servidor." } else { "Este nombre sólo cambia en tu lista. No renombra el servidor del anfitrión." });
                ui.add(egui::TextEdit::singleline(&mut self.rename_draft).char_limit(64).desired_width(ui.available_width()));
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.rename_draft.trim().is_empty(), egui::Button::new("Guardar nombre")).clicked() {
                        self.command(Command::RenameServer { id: server.id.clone(), name: self.rename_draft.trim().into() }); self.rename_target = None;
                    }
                    if ui.button("Cancelar").clicked() { self.rename_target = None; }
                });
            });
            if response.should_close() {
                self.rename_target = None;
            }
        }
        if let Some(server) = self.delete_target.clone() {
            let response = egui::Modal::new(egui::Id::unique("delete_server")).show(ctx, |ui| {
                ui.set_width(390.0); ui.heading("Borrar servidor");
                ui.add(egui::Label::new(format!("¿Borrar «{}»?", server.display_name())).wrap());
                Self::secondary(ui, if server.owned { "Se apagará si está activo y se borrarán su identidad, invitaciones e historial local permanente. Las invitaciones anteriores dejarán de servir. No se puede deshacer." } else { "Se quitará de tu lista y se cerrará tu conexión si está activa. El servidor del anfitrión seguirá funcionando. Para volver necesitarás una invitación." });
                ui.horizontal(|ui| {
                    if ui.button("Cancelar").clicked() { self.delete_target = None; }
                    if ui.button(RichText::new("Borrar servidor").color(ERROR)).clicked() {
                        self.command(Command::DeleteServer { id: server.id.clone() }); self.delete_target = None;
                    }
                });
            });
            if response.should_close() {
                self.delete_target = None;
            }
        }

        if self.host_open {
            let response = egui::Modal::new(egui::Id::unique("create_server")).show(ctx, |ui| {
                ui.set_width(390.0);
                ui.heading("Crear servidor");
                egui::ScrollArea::vertical().id_salt("create_server_form").max_height((ctx.viewport_rect().height() - 180.0).max(220.0)).show(ui, |ui| {
                Self::secondary(ui, "Tu equipo debe permanecer abierto y accesible por UDP.");
                Self::secondary(ui, "Para conectar desde otra casa necesitás una IP pública alcanzable y redirigir el puerto UDP del router. NIP todavía no configura el router ni el firewall automáticamente.");
                ui.label("Tu nombre");
                ui.add(egui::TextEdit::singleline(&mut self.user).char_limit(64).desired_width(ui.available_width()));
                ui.label("Nombre del servidor");
                ui.add(egui::TextEdit::singleline(&mut self.host_name).char_limit(64).desired_width(ui.available_width()));
                ui.add_space(12.0);
                ui.label(RichText::new("Tipo de servidor").strong());
                ui.radio_value(&mut self.host_persistent, false, "De sesión · chat temporal");
                ui.radio_value(&mut self.host_persistent, true, "Persistente · datos en este equipo");
                Self::secondary(ui, if self.host_persistent { "Guarda mensajes y configuración. Se inicia al abrir NIP; podés cambiarlo en Ajustes → Servidores. Sin NIP abierto estará fuera de línea." } else { "Se guarda el servidor para volver a encenderlo. Su chat se pierde al apagarlo. Podés convertirlo en persistente después." });
                ui.add_space(12.0);
                ui.label("Puerto UDP");
                ui.add(egui::TextEdit::singleline(&mut self.port).char_limit(5).desired_width(ui.available_width()));
                });
                let port = self.port.parse::<u16>().ok().filter(|p| *p != 0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            port.is_some()
                                && !self.user.trim().is_empty()
                                && !self.host_name.trim().is_empty()
                                && !self.busy,
                            egui::Button::new("Crear servidor"),
                        )
                        .clicked()
                    {
                        self.busy = true;
                        self.command(Command::Create {
                            persistent: self.host_persistent,
                            name: self.host_name.trim().into(),
                            port: port.unwrap(),
                            user: self.user.trim().into(),
                        });
                        self.host_open = false;
                    }
                    if ui.button("Cancelar").clicked() {
                        self.host_open = false;
                    }
                });
                if port.is_none() {
                    Self::secondary(ui, "Ingresá un puerto entre 1 y 65535.");
                }
            });
            if response.should_close() {
                self.host_open = false;
            }
        }
        if self.connect_open {
            let response = egui::Modal::new(egui::Id::unique("connect_server")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Conectar con invitación");
                ui.label("Tu nombre");
                ui.add(egui::TextEdit::singleline(&mut self.user).char_limit(64));
                ui.label("Invitación privada");
                ui.add(
                    egui::TextEdit::multiline(&mut self.invitation)
                        .hint_text("nip-native://join#…")
                        .char_limit(16384)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                );
                Self::secondary(
                    ui,
                    "Usá una invitación de NIP nativo. Los clientes Electron no son compatibles.",
                );
                if let Ok(invite) = crate::storage::Invitation::parse(self.invitation.trim()) {
                    endpoint_help(ui, &invite.address);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.user.trim().is_empty()
                                && !self.invitation.trim().is_empty()
                                && !self.busy,
                            egui::Button::new("Conectar"),
                        )
                        .clicked()
                    {
                        self.busy = true;
                        self.command(Command::Connect {
                            invitation: self.invitation.trim().into(),
                            user: self.user.trim().into(),
                        });
                        self.invitation.clear();
                        self.connect_open = false;
                    }
                    if ui.button("Cancelar").clicked() {
                        self.invitation.clear();
                        self.connect_open = false;
                    }
                });
            });
            if response.should_close() {
                self.invitation.clear();
                self.connect_open = false;
            }
        }
        if self.invite_open {
            let response = egui::Modal::new(egui::Id::unique("invite_people"))
                .area(egui::Modal::default_area(egui::Id::unique("invite_people")).default_height((ctx.content_rect().height() - 80.0).max(200.0)))
                .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 80.0).max(200.0)).show(ui, |ui| {
                ui.set_width(460.0); ui.heading("Invitar personas");
                ui.label("Dirección alcanzable del anfitrión (IP:puerto)");
                if ui.add(egui::TextEdit::singleline(&mut self.endpoint).hint_text("IP pública o de la red:puerto UDP")).changed() { self.invitation_result = None; }
                if let Some(host) = self.state.hosts.iter().find(|h| self.invite_target.as_ref() == Some(&h.id)).cloned() {
                    for address in host.addresses {
                        let label = format!("{} · {}", address, if address.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback()) { "Sólo esta PC" } else { "Red local / VPN" });
                        if ui.selectable_label(self.endpoint == address, label).clicked() { self.endpoint = address; self.invitation_result = None; }
                    }
                }
                endpoint_help(ui, &self.endpoint);
                Self::secondary(ui, "Entre casas: usá la IP pública del anfitrión y redirigí UDP en el router al puerto de NIP. Permitir la app en el firewall es un paso separado. Con CGNAT, esta conexión directa puede no ser posible.");
                Self::secondary(ui, "NIP no abre puertos del router automáticamente ni incluye relay. Generar una invitación no comprueba acceso desde Internet.");
                Self::secondary(ui, "Compartila sólo con personas de confianza. Incluye permiso de acceso al servidor.");
                if ui.add_enabled(self.endpoint.trim().parse::<std::net::SocketAddr>().is_ok_and(|a| a.port() != 0 && !a.ip().is_unspecified()) && !self.busy, egui::Button::new("Generar invitación")).clicked() {
                    self.invitation_result = None; self.busy = true; self.command(Command::Invite { id: self.invite_target.clone(), endpoint: self.endpoint.trim().into() });
                }
                if let Some(invitation) = &self.invitation_result {
                    ui.label("Invitación lista");
                    if Self::primary(ui, "Copiar invitación").clicked() { ctx.copy_text(invitation.clone()); }
                }
                if ui.button("Cerrar").clicked() { self.invite_open = false; self.invitation_result = None; }
                });
            });
            if response.should_close() {
                self.invite_open = false;
                self.invitation_result = None;
            }
        }
    }
}
fn suggested_endpoint(addresses: &[String]) -> String {
    addresses
        .iter()
        .find(|address| {
            address
                .parse::<std::net::SocketAddr>()
                .is_ok_and(|a| !a.ip().is_loopback() && !a.ip().is_unspecified())
        })
        .cloned()
        .unwrap_or_default()
}
fn endpoint_help(ui: &mut egui::Ui, endpoint: &str) {
    if let Ok(address) = endpoint.trim().parse::<std::net::SocketAddr>() {
        if address.ip().is_loopback() {
            ui.colored_label(ERROR, "Esta dirección apunta a la PC de quien abre la invitación. Sólo funciona para pruebas en un mismo equipo.");
        } else if match address.ip() {
            std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
            std::net::IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
        } {
            NipApp::secondary(
                ui,
                "Es una dirección de red local: sirve en la misma red o VPN, no directamente desde otra casa.",
            );
        }
    }
}
// Voice RMS is usually well below 1. A logarithmic meter keeps quiet speech visible.
fn meter_level(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 0.001 {
        0.0
    } else {
        ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    }
}
impl eframe::App for NipApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe runs logic while hidden, so background audio events cannot accumulate
        // for the entire time the window is minimized.
        self.poll();
        if self
            .quit_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.command(Command::Shutdown);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(
            if self.audio.testing || self.audio.calling {
                100
            } else {
                200
            },
        ));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.command(Command::Shutdown);
    }
}
impl Drop for NipApp {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}
#[derive(Clone, Copy)]
enum Icon {
    Mic,
    Headphones,
    Settings,
}
fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let response = ui.add(
        egui::Button::new("")
            .min_size(Vec2::splat(36.0))
            .selected(selected),
    );
    let rect = response.rect;
    response.widget_info(|| {
        if matches!(icon, Icon::Settings) {
            egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), label)
        } else {
            egui::WidgetInfo::selected(egui::Role::Button, response.enabled(), selected, label)
        }
    });
    let stroke = Stroke::new(
        1.7,
        if !response.enabled() {
            SECONDARY
        } else if selected {
            ACCENT
        } else {
            TEXT
        },
    );
    let painter = ui.painter_at(rect);
    let center = rect.center();
    if response.hovered() || response.has_focus() || selected {
        painter.rect_stroke(rect.shrink(1.0), 6.0, stroke, egui::StrokeKind::Inside);
    }
    match icon {
        Icon::Mic => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center - egui::vec2(0.0, 3.0), egui::vec2(7.0, 13.0)),
                4.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-7.0, -2.0),
                    center + egui::vec2(-7.0, 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(7.0, -2.0),
                    center + egui::vec2(7.0, 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-7.0, 4.0),
                    center + egui::vec2(0.0, 8.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(7.0, 4.0), center + egui::vec2(0.0, 8.0)],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(0.0, 8.0),
                    center + egui::vec2(0.0, 11.0),
                ],
                stroke,
            );
        }
        Icon::Headphones => {
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-9.0, 6.0),
                    center + egui::vec2(-9.0, -3.0),
                    center + egui::vec2(-5.0, -9.0),
                    center + egui::vec2(5.0, -9.0),
                    center + egui::vec2(9.0, -3.0),
                    center + egui::vec2(9.0, 6.0),
                ],
                stroke,
            ));
            for x in [-7.0, 7.0] {
                painter.rect_stroke(
                    egui::Rect::from_center_size(
                        center + egui::vec2(x, 4.0),
                        egui::vec2(5.0, 10.0),
                    ),
                    2.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Settings => {
            painter.circle_stroke(center, 8.0, stroke);
            painter.circle_stroke(center, 3.0, stroke);
            for step in 0..8 {
                let angle = step as f32 * std::f32::consts::TAU / 8.0;
                let dir = egui::vec2(angle.cos(), angle.sin());
                painter.line_segment([center + dir * 8.0, center + dir * 11.0], stroke);
            }
        }
    }
    if selected && !matches!(icon, Icon::Settings) {
        painter.line_segment(
            [
                rect.left_bottom() + egui::vec2(7.0, -7.0),
                rect.right_top() + egui::vec2(-7.0, 7.0),
            ],
            stroke,
        );
    }
    response.on_hover_text(label)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_channel_switch_restores_each_draft_without_touching_audio() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, receiver);
        let ctx = egui::Context::default();
        configure(&ctx);
        let state = |selected: &str| AppState {
            room: Some(Room {
                storage_usage: Default::default(),
                protocol_version: 2,
                channels: vec![
                    Channel {
                        id: "general".into(),
                        name: "general".into(),
                        kind: ChannelKind::Text,
                    },
                    Channel {
                        id: "second".into(),
                        name: "segundo".into(),
                        kind: ChannelKind::Text,
                    },
                ],
                selected_text: selected.into(),
                persistent: false,
                has_older_messages: false,
                id: "server".into(),
                name: "QA".into(),
                self_id: 1,
                members: vec![],
                history: vec![],
            }),
            ..Default::default()
        };
        events.send(Event::State(state("general"))).unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        app.draft = "borrador general".into();
        events.send(Event::State(state("second"))).unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        assert!(app.draft.is_empty());
        app.draft = "borrador segundo".into();
        events.send(Event::State(state("general"))).unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        assert_eq!(app.draft, "borrador general");
        events.send(Event::State(state("second"))).unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        assert_eq!(app.draft, "borrador segundo");
        assert!(
            received.try_recv().is_err(),
            "seleccionar texto no modifica audio"
        );
    }
    #[test]
    fn clicking_voice_channel_emits_one_explicit_join_at_minimum_viewport() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, receiver);
        let ctx = egui::Context::default();
        configure(&ctx);
        ctx.enable_accesskit();
        events
            .send(Event::State(AppState {
                room: Some(Room {
                    storage_usage: Default::default(),
                    protocol_version: 2,
                    channels: default_channels(),
                    selected_text: "general".into(),
                    persistent: false,
                    has_older_messages: false,
                    id: "server".into(),
                    name: "QA".into(),
                    self_id: 1,
                    members: vec![],
                    history: vec![],
                }),
                ..Default::default()
            }))
            .unwrap();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760.0, 540.0),
                )),
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        let tree = output.platform_output.accesskit_update.as_ref().unwrap();
        let bounds = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("General · voz"))
            .unwrap()
            .1
            .bounds()
            .unwrap();
        assert!(bounds.x0 >= 0.0 && bounds.x1 <= 760.0 && bounds.y1 <= 540.0);
        let position = egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        output.textures_delta.clear();
        assert!(
            received.try_recv().is_err(),
            "mostrar canales no abre captura"
        );
        for pressed in [true, false] {
            let mut output =
                ctx.run_ui(pointer_input(position, Some(pressed)), |ui| app.render(ui));
            output.textures_delta.clear();
        }
        assert!(
            matches!(received.try_recv(), Ok(Command::JoinVoice { channel_id }) if channel_id == GENERAL_VOICE_CHANNEL)
        );
        assert!(received.try_recv().is_err());
    }
    #[test]
    fn long_server_name_leaves_menu_inside_fixed_row_width() {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        app.user = "QA".into();
        let server = SavedServer {
            local: None,
            id: "server".into(),
            name: "LongName".repeat(8),
            endpoint: "127.0.0.1:45454".into(),
            alias: None,
            owned: false,
        };
        let ctx = egui::Context::default();
        configure(&ctx);
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(156.0, 80.0),
                )),
                ..Default::default()
            },
            |ui| app.server_row(ui, &server),
        );
        let expected = format!("Opciones de {}", server.name);
        let tree = output.platform_output.accesskit_update.as_ref().unwrap();
        let menu = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(expected.as_str()))
            .unwrap()
            .1
            .bounds()
            .unwrap();
        assert!(
            menu.x1 <= 156.0 && menu.x1 - menu.x0 >= 28.0,
            "menu clipped by name: {menu:?}"
        );
        output.textures_delta.clear();
    }
    #[test]
    fn settings_entry_points_route_and_leaving_audio_stops_the_test() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        app.open_settings(SettingsPage::Profile);
        assert!(app.settings_open && app.settings_page == SettingsPage::Profile);
        assert!(received.try_recv().is_err());
        app.open_settings(SettingsPage::Audio);
        assert!(matches!(received.try_recv(), Ok(Command::RefreshDevices)));
        app.audio.testing = true;
        app.open_settings(SettingsPage::Permissions);
        assert!(matches!(received.try_recv(), Ok(Command::StopTest)));
        assert!(
            received.try_recv().is_err(),
            "no navigation starts microphone capture"
        );
    }
    #[test]
    fn invite_never_suggests_loopback_for_another_person() {
        assert_eq!(
            suggested_endpoint(&["127.0.0.1:45454".into(), "192.168.1.50:45454".into()]),
            "192.168.1.50:45454"
        );
        assert!(suggested_endpoint(&["127.0.0.1:45454".into(), "[::1]:45454".into()]).is_empty());
    }
    #[test]
    fn server_switch_discards_draft_but_channel_navigation_preserves_it() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, receiver);
        let room = |id: &str| Room {
            storage_usage: Default::default(),
            protocol_version: 2,
            channels: default_channels(),
            selected_text: GENERAL_TEXT_CHANNEL.into(),
            persistent: false,
            has_older_messages: false,
            id: id.into(),
            name: id.into(),
            self_id: 1,
            members: vec![],
            history: vec![],
        };
        app.state.room = Some(room("A"));
        app.draft = "privado para A".into();
        app.room_view = RoomView::Voice;
        events
            .send(Event::State(AppState {
                room: Some(room("A")),
                connecting: true,
                ..Default::default()
            }))
            .unwrap();
        app.poll();
        assert!(app.busy);
        assert_eq!(app.draft, "privado para A");
        assert!(app.room_view == RoomView::Voice);
        events
            .send(Event::State(AppState {
                room: Some(room("B")),
                ..Default::default()
            }))
            .unwrap();
        app.poll();
        assert!(!app.busy);
        assert!(app.draft.is_empty());
        assert!(app.room_view == RoomView::Chat);
        assert!(received.try_recv().is_err(), "navegar no activa captura");
    }
    #[test]
    fn quiet_speech_is_visible_on_meter_without_showing_silence_as_signal() {
        assert_eq!(meter_level(0.0), 0.0);
        assert_eq!(meter_level(f32::NAN), 0.0);
        assert!(meter_level(0.01) > 0.3);
        assert_eq!(meter_level(1.0), 1.0);
    }
    #[test]
    fn equalizer_and_settings_navigation_fit_small_windows() {
        let ctx = egui::Context::default();
        configure(&ctx);
        for width in [360.0, 480.0, 640.0] {
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1100.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let id = ui.make_persistent_id("eq_bounds");
                        let mut state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                true,
                            );
                        state.set_open(true);
                        state.store(ui.ctx());
                        let available = ui.available_width();
                        let mut settings = EqualizerSettings {
                            enabled: true,
                            ..Default::default()
                        };
                        equalizer_settings(ui, "EQ", "eq_bounds", &mut settings);
                        assert!(
                            ui.min_rect().width() <= available + 1.0,
                            "EQ overflow at {width}: {} > {available}",
                            ui.min_rect().width()
                        );
                    },
                );
                output.textures_delta.clear();
            }
        }
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        for page in [
            SettingsPage::Profile,
            SettingsPage::Audio,
            SettingsPage::Calls,
            SettingsPage::Servers,
            SettingsPage::Application,
            SettingsPage::Connection,
        ] {
            app.settings_open = true;
            app.settings_page = page;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760.0, 540.0),
                    )),
                    ..Default::default()
                },
                |ui| app.render(ui),
            );
            assert!(!output.shapes.is_empty());
            output.textures_delta.clear();
        }
    }
    #[test]
    fn server_creation_actions_remain_visible_in_small_windows() {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        app.host_open = true;
        let ctx = egui::Context::default();
        configure(&ctx);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 540.0));
        let mut cancel = None;
        for _ in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| app.render(ui),
            );
            cancel = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Cancelar" => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            });
            output.textures_delta.clear();
        }
        let cancel = cancel.expect("visible cancel action after modal layout settles");
        assert!(
            viewport.contains_rect(cancel),
            "create dialog footer must fit the minimum viewport: {cancel:?}"
        );
    }
    #[test]
    fn audio_settings_page_is_painted_inside_the_viewport() {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        app.settings_open = true;
        app.settings_page = SettingsPage::Audio;
        let ctx = egui::Context::default();
        configure(&ctx);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0));
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| app.render(ui),
            );
            let title = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Voz y audio" => {
                    Some((shape.clip_rect, text.visual_bounding_rect()))
                }
                _ => None,
            });
            output.textures_delta.clear();
            if let Some((clip, text)) = title {
                assert!(
                    viewport.intersects(text) && clip.intersects(text),
                    "dialog title outside visible area: {text:?}, clip {clip:?}"
                );
                return;
            }
        }
        panic!("audio dialog has no painted title");
    }
    fn help_frame(
        app: &mut NipApp,
        ctx: &egui::Context,
        input: egui::RawInput,
    ) -> (egui::Response, bool) {
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| {
            response = Some(app.help_button(ui, "Entrada", "Ayuda de entrada compartida"));
            let _ = ui.button("Otro ajuste");
        });
        let shown = output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "Ayuda de entrada compartida"));
        output.textures_delta.clear();
        (response.unwrap(), shown)
    }
    fn key_input(key: egui::Key) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        input
    }
    fn pointer_input(position: egui::Pos2, pressed: Option<bool>) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(position));
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        input
    }
    #[test]
    fn same_anchored_help_supports_hover_tab_click_escape_and_outside_click() {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        let ctx = egui::Context::default();
        configure(&ctx);
        let (button, _) = help_frame(&mut app, &ctx, Default::default());
        help_frame(&mut app, &ctx, pointer_input(button.rect.center(), None));
        assert!(help_frame(&mut app, &ctx, Default::default()).1);
        help_frame(
            &mut app,
            &ctx,
            pointer_input(egui::pos2(600.0, 400.0), None),
        );
        assert!(!help_frame(&mut app, &ctx, Default::default()).1);
        help_frame(&mut app, &ctx, key_input(egui::Key::Tab));
        let (focused, shown) = help_frame(&mut app, &ctx, Default::default());
        assert!(focused.has_focus() && shown);
        help_frame(&mut app, &ctx, key_input(egui::Key::Escape));
        assert!(!help_frame(&mut app, &ctx, Default::default()).1);
        help_frame(
            &mut app,
            &ctx,
            pointer_input(button.rect.center(), Some(true)),
        );
        help_frame(
            &mut app,
            &ctx,
            pointer_input(button.rect.center(), Some(false)),
        );
        help_frame(
            &mut app,
            &ctx,
            pointer_input(egui::pos2(600.0, 400.0), None),
        );
        assert!(
            help_frame(&mut app, &ctx, Default::default()).1,
            "clic fija la misma ayuda al apartar el puntero"
        );
        help_frame(
            &mut app,
            &ctx,
            pointer_input(egui::pos2(600.0, 400.0), Some(true)),
        );
        help_frame(
            &mut app,
            &ctx,
            pointer_input(egui::pos2(600.0, 400.0), Some(false)),
        );
        assert!(!help_frame(&mut app, &ctx, Default::default()).1);
    }
    #[test]
    fn private_test_and_deafen_disable_mic_with_native_toggle_state() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        let ctx = egui::Context::default();
        configure(&ctx);
        ctx.enable_accesskit();
        for (testing, deafened) in [(true, false), (false, true)] {
            app.audio.testing = testing;
            app.audio.deafened = deafened;
            app.audio.muted = true;
            let mut output = ctx.run_ui(Default::default(), |ui| app.voice_bar(ui));
            let tree = output.platform_output.accesskit_update.as_ref().unwrap();
            let mic = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Activar micrófono"))
                .map(|(_, node)| node)
                .unwrap();
            assert!(mic.is_disabled());
            assert_eq!(mic.toggled(), Some(egui::accesskit::Toggled::True));
            let bounds = mic.bounds().unwrap();
            let position = egui::pos2(
                ((bounds.x0 + bounds.x1) / 2.0) as f32,
                ((bounds.y0 + bounds.y1) / 2.0) as f32,
            );
            output.textures_delta.clear();
            for pressed in [true, false] {
                let mut output = ctx.run_ui(pointer_input(position, Some(pressed)), |ui| {
                    app.voice_bar(ui)
                });
                output.textures_delta.clear();
            }
            assert!(
                received.try_recv().is_err(),
                "el control deshabilitado no emite Mute ni reabre envío"
            );
        }
    }
    #[test]
    fn keyboard_chat_and_disconnect_do_not_start_capture() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, receiver);
        let ctx = egui::Context::default();
        configure(&ctx);
        events
            .send(Event::State(AppState {
                room: Some(Room {
                    storage_usage: Default::default(),
                    protocol_version: 2,
                    channels: default_channels(),
                    selected_text: GENERAL_TEXT_CHANNEL.into(),
                    persistent: false,
                    has_older_messages: false,
                    id: "r".into(),
                    name: "Sala de prueba".into(),
                    self_id: 1,
                    members: vec![],
                    history: vec![],
                }),
                ..Default::default()
            }))
            .unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        assert!(!output.shapes.is_empty());
        // Headless tests have no texture renderer; acknowledge deltas explicitly.
        output.textures_delta.clear();
        assert!(
            received.try_recv().is_err(),
            "abrir una sala no inicia captura"
        );
        app.draft = "Hola al grupo".into();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("chat_composer")));
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        let mut output = ctx.run_ui(input, |ui| app.render(ui));
        output.textures_delta.clear();
        assert!(
            matches!(received.try_recv(), Ok(Command::SendChat { server_id, channel_id, text }) if server_id == "r" && channel_id == "general" && text == "Hola al grupo")
        );
        events
            .send(Event::Disconnected("Servidor cerrado".into()))
            .unwrap();
        let mut output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        app.draft = "No enviar".into();
        app.send_chat();
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn chat_is_unicode_bounded_and_requires_a_room() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (_, events) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, events);
        app.draft = "hola".into();
        app.send_chat();
        assert!(received.try_recv().is_err());
        app.state.room = Some(Room {
            storage_usage: Default::default(),
            protocol_version: 2,
            channels: default_channels(),
            selected_text: GENERAL_TEXT_CHANNEL.into(),
            persistent: false,
            has_older_messages: false,
            id: "room".into(),
            name: "Amigos".into(),
            self_id: 1,
            members: vec![],
            history: vec![],
        });
        app.draft = "ñ".repeat(2001);
        app.send_chat();
        assert!(received.try_recv().is_err());
        app.draft = "ñ".repeat(2000);
        app.send_chat();
        assert!(
            matches!(received.try_recv(), Ok(Command::SendChat { text, .. }) if text.chars().count() == 2000)
        );
        assert!(app.draft.is_empty());
    }
}
