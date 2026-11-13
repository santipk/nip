use crate::contracts::*;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;

const BG: Color32 = Color32::from_rgb(17, 23, 21);
const SIDE: Color32 = Color32::from_rgb(25, 33, 28);
const TEXT: Color32 = Color32::from_rgb(237, 241, 232);
const SECONDARY: Color32 = Color32::from_rgb(155, 169, 159);
const ACCENT: Color32 = Color32::from_rgb(196, 237, 145);

pub fn configure(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = SIDE;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = Stroke::new(1.0, BG);
    style.visuals.widgets.inactive.bg_fill = SIDE;
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
    user: String,
    host_name: String,
    port: String,
    invitation: String,
    endpoint: String,
    invitation_result: Option<String>,
    draft: String,
    error: Option<String>,
    settings_open: bool,
    host_open: bool,
    connect_open: bool,
    invite_open: bool,
    help: Option<(&'static str, &'static str)>,
    busy: bool,
    test_pending: bool,
    pub quit_at: Option<Instant>,
}

impl NipApp {
    pub fn new(commands: UnboundedSender<Command>, events: Receiver<Event>) -> Self {
        Self {
            commands,
            events,
            state: AppState::default(),
            audio: AudioStatus::default(),
            audio_draft: AudioSettings::default(),
            user: String::new(),
            host_name: String::new(),
            port: "45454".into(),
            invitation: String::new(),
            endpoint: String::new(),
            invitation_result: None,
            draft: String::new(),
            error: None,
            settings_open: false,
            host_open: false,
            connect_open: false,
            invite_open: false,
            help: None,
            busy: false,
            test_pending: false,
            quit_at: None,
        }
    }
    fn command(&mut self, command: Command) {
        if self.commands.send(command).is_err() {
            self.error = Some("El servicio se cerró. Cerrá y volvé a abrir NIP.".into());
            self.busy = false;
            self.test_pending = false;
        }
    }
    fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::State(state) => {
                    self.state = state;
                    self.busy = false;
                }
                Event::Members(members) => {
                    if let Some(room) = &mut self.state.room {
                        room.members = members;
                    }
                }
                Event::Chat(message) => {
                    if let Some(room) = &mut self.state.room {
                        if !room.history.iter().any(|m| m.id == message.id) {
                            room.history.push(message);
                            if room.history.len() > 100 {
                                room.history.remove(0);
                            }
                        }
                    }
                }
                Event::Audio(audio) => {
                    self.audio_draft = audio.settings.clone();
                    self.audio = audio;
                    self.test_pending = false;
                }
                Event::Invitation(invitation) => {
                    self.invitation_result = Some(invitation);
                    self.busy = false;
                }
                Event::Error(error) => {
                    self.error = Some(error);
                    self.busy = false;
                    self.test_pending = false;
                }
                Event::Disconnected(reason) => {
                    self.state.room = None;
                    self.draft.clear();
                    self.busy = false;
                    self.error = Some(reason);
                }
            }
        }
    }
    fn send_chat(&mut self) {
        let text = self.draft.trim();
        if self.state.room.is_some() && !text.is_empty() && text.chars().count() <= 2000 {
            let command = Command::Chat(text.into());
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
        if self
            .quit_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.command(Command::Shutdown);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // A quiet idle window repaints only to receive background state, never to capture audio.
        ctx.request_repaint_after(Duration::from_millis(100));
        egui::Panel::left("servers")
            .exact_size(232.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDE).inner_margin(20))
            .show(ui, |ui| {
                ui.label(RichText::new("NIP").size(27.0).strong());
                ui.add_space(24.0);
                Self::secondary(ui, "Tu nombre");
                ui.add(
                    egui::TextEdit::singleline(&mut self.user)
                        .hint_text("Cómo te ven los demás")
                        .char_limit(64)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(24.0);
                ui.label(RichText::new("Servidores").strong());
                let saved = self.state.saved.clone();
                if saved.is_empty() {
                    Self::secondary(ui, "Los servidores que conectes aparecerán acá.");
                }
                for server in saved {
                    let selected = self
                        .state
                        .room
                        .as_ref()
                        .is_some_and(|room| room.id == server.id);
                    let response = ui.add_enabled(
                        !self.busy && !self.user.trim().is_empty(),
                        egui::Button::new(&server.name).selected(selected),
                    );
                    if response
                        .on_hover_text(format!("Reconectar a {}", server.endpoint))
                        .clicked()
                    {
                        self.busy = true;
                        self.command(Command::Reconnect {
                            id: server.id,
                            user: self.user.trim().into(),
                        });
                    }
                }
                ui.add_space(20.0);
                if ui.button("Conectar con invitación").clicked() {
                    self.connect_open = true;
                }
                if ui.button("Crear servidor").clicked() {
                    self.host_open = true;
                }
                if self.busy {
                    Self::secondary(ui, "Conectando…");
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    Self::secondary(ui, "Nativo · protocolo QUIC");
                    if let Some(host) = self.state.host.clone() {
                        if ui.button("Detener servidor").clicked() {
                            self.command(Command::StopHost);
                        }
                        if ui.button("Invitar personas").clicked() {
                            self.invite_open = true;
                            self.invitation_result = None;
                            self.endpoint = host.addresses.first().cloned().unwrap_or_default();
                        }
                        Self::secondary(ui, format!("Anfitrión · {}", host.name));
                    }
                });
            });
        egui::Panel::bottom("voice_controls")
            .frame(egui::Frame::new().fill(SIDE).inner_margin(16))
            .show(ui, |ui| self.voice_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(28))
            .show(ui, |ui| {
                if let Some(error) = self.error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("Atención: {error}")).color(ACCENT));
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
    fn voice_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let status = if self.audio.testing {
                "Prueba privada · sin envío"
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
            if icon_button(
                ui,
                Icon::Mic,
                if self.audio.muted {
                    "Activar micrófono"
                } else {
                    "Silenciar micrófono"
                },
                self.audio.muted,
            )
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
                self.settings_open = true;
                self.command(Command::RefreshDevices);
            }
        });
    }
    fn room(&mut self, ui: &mut egui::Ui, room: Room) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&room.name).size(26.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Desconectar").clicked() {
                    self.command(Command::Disconnect);
                }
            });
        });
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            for member in &room.members {
                let status = if member.calling {
                    if member.muted {
                        "micrófono silenciado"
                    } else {
                        "en llamada"
                    }
                } else {
                    "sin micrófono"
                };
                ui.label(RichText::new(&member.name).strong())
                    .on_hover_text(status);
                Self::secondary(ui, status);
            }
        });
        ui.add_space(20.0);
        let composer_height = 74.0;
        egui::ScrollArea::vertical()
            .id_salt("chat_history")
            .stick_to_bottom(true)
            .max_height((ui.available_height() - composer_height).max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
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
                    ui.add_space(12.0);
                }
            });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.draft)
                    .id(egui::Id::unique("chat_composer"))
                    .hint_text("Mensaje al grupo")
                    .char_limit(2000)
                    .desired_width((ui.available_width() - 90.0).max(80.0)),
            );
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
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
            format!("{} / 2000 · Enter para enviar", self.draft.chars().count()),
        );
    }

    fn help_button(&mut self, ui: &mut egui::Ui, title: &'static str, text: &'static str) {
        let response = ui.button("i").on_hover_text(text);
        if response.has_focus() {
            egui::Tooltip::for_widget(&response).show(|ui| {
                ui.label(text);
            });
        }
        if response.clicked() {
            self.help = Some((title, text));
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.set_width(520.0);
        ui.heading("Audio");
        Self::secondary(ui, "Preferencias locales. Se aplican dentro de NIP.");
        ui.add_space(16.0);
        let before = self.audio_draft.clone();
        egui::Grid::new("audio_rows").num_columns(3).spacing([16.0, 16.0]).show(ui, |ui| {
            ui.label("Entrada");
            egui::ComboBox::from_id_salt("input_device").selected_text(if self.audio_draft.input.is_empty() { "Predeterminada" } else { &self.audio_draft.input }).width(250.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.audio_draft.input, String::new(), "Predeterminada");
                for device in &self.audio.inputs { ui.selectable_value(&mut self.audio_draft.input, device.clone(), device); }
            });
            self.help_button(ui, "Entrada", "Elegí el micrófono que usa NIP. Cambiarlo detiene la prueba privada; no inicia captura fuera de llamada."); ui.end_row();
            ui.label("Salida");
            egui::ComboBox::from_id_salt("output_device").selected_text(if self.audio_draft.output.is_empty() { "Predeterminada" } else { &self.audio_draft.output }).width(250.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.audio_draft.output, String::new(), "Predeterminada");
                for device in &self.audio.outputs { ui.selectable_value(&mut self.audio_draft.output, device.clone(), device); }
            });
            self.help_button(ui, "Salida", "Dispositivo para escuchar al grupo y tu prueba privada. No cambia la salida global de Windows."); ui.end_row();
            ui.label("Nivel del micrófono");
            let mut level = self.audio_draft.input_level * 100.0;
            if ui.add(egui::Slider::new(&mut level, 0.0..=100.0).suffix(" %")).changed() { self.audio_draft.input_level = level / 100.0; }
            self.help_button(ui, "Nivel del micrófono", "Ajusta la señal dentro de NIP, también en la prueba privada. No modifica la ganancia física ni el nivel de Windows."); ui.end_row();
            ui.label("Volumen de la app");
            let mut volume = self.audio_draft.volume * 100.0;
            if ui.add(egui::Slider::new(&mut volume, 0.0..=100.0).suffix(" %")).changed() { self.audio_draft.volume = volume / 100.0; }
            self.help_button(ui, "Volumen de la app", "Controla lo que escuchás en NIP, incluida la prueba. No cambia el volumen global de Windows."); ui.end_row();
        });
        if self.audio_draft != before {
            self.command(Command::AudioSettings(self.audio_draft.clone()));
        }
        ui.add_space(20.0);
        ui.label(RichText::new("Prueba privada").strong());
        Self::secondary(
            ui,
            "Usá auriculares para evitar acople. Tu voz no se envía al grupo.",
        );
        ui.add(
            egui::ProgressBar::new(self.audio.level.clamp(0.0, 1.0))
                .text(if self.audio.testing {
                    "Nivel del micrófono · prueba activa"
                } else {
                    "Nivel del micrófono"
                })
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            let label = if self.test_pending {
                "Esperando audio…"
            } else if self.audio.testing {
                "Detener prueba"
            } else {
                "Probar micrófono"
            };
            if ui
                .add_enabled(
                    !self.test_pending && (!self.audio.deafened || self.audio.testing),
                    egui::Button::new(label),
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
            if ui.button("Actualizar dispositivos").clicked() {
                self.command(Command::RefreshDevices);
            }
        });
        if self.audio.deafened {
            Self::secondary(ui, "Desactivá Ensordecer para probar el micrófono.");
        }
        if self.audio.testing {
            ui.label(
                RichText::new("Escucha local activa · envío al grupo suspendido").color(ACCENT),
            );
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui.button("Restablecer audio").clicked() {
                self.command(Command::AudioSettings(AudioSettings::default()));
            }
            if ui.button("Listo").clicked() {
                self.settings_open = false;
            }
        });
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.settings_open {
            let response = egui::Modal::new(egui::Id::unique("audio_settings"))
                .show(ctx, |ui| self.settings(ui));
            if response.should_close() && self.help.is_none() {
                self.settings_open = false;
            }
            if !self.settings_open && (self.audio.testing || self.test_pending) {
                self.command(Command::StopTest);
            }
        }
        if self.host_open {
            let response = egui::Modal::new(egui::Id::unique("create_server")).show(ctx, |ui| {
                ui.set_width(390.0);
                ui.heading("Crear servidor");
                Self::secondary(ui, "Tu equipo debe permanecer abierto y accesible por UDP.");
                ui.label("Tu nombre");
                ui.text_edit_singleline(&mut self.user);
                ui.label("Nombre del servidor");
                ui.add(egui::TextEdit::singleline(&mut self.host_name).char_limit(64));
                ui.label("Puerto UDP");
                ui.add(egui::TextEdit::singleline(&mut self.port).char_limit(5));
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
            let response = egui::Modal::new(egui::Id::unique("invite_people")).show(ctx, |ui| {
                ui.set_width(460.0); ui.heading("Invitar personas");
                ui.label("Dirección alcanzable del anfitrión (IP:puerto)");
                if ui.text_edit_singleline(&mut self.endpoint).changed() { self.invitation_result = None; }
                if let Some(host) = self.state.host.clone() {
                    for address in host.addresses {
                        if ui.selectable_label(self.endpoint == address, &address).clicked() { self.endpoint = address; self.invitation_result = None; }
                    }
                }
                Self::secondary(ui, "Compartila sólo con personas de confianza. Incluye permiso de acceso al servidor.");
                if ui.add_enabled(!self.endpoint.trim().is_empty() && !self.busy, egui::Button::new("Generar invitación")).clicked() {
                    self.invitation_result = None; self.busy = true; self.command(Command::Invite { endpoint: self.endpoint.trim().into() });
                }
                if let Some(invitation) = &self.invitation_result {
                    ui.label("Invitación lista");
                    if Self::primary(ui, "Copiar invitación").clicked() { ctx.copy_text(invitation.clone()); }
                }
                if ui.button("Cerrar").clicked() { self.invite_open = false; self.invitation_result = None; }
            });
            if response.should_close() {
                self.invite_open = false;
                self.invitation_result = None;
            }
        }
        if let Some((title, text)) = self.help {
            let response = egui::Modal::new(egui::Id::unique("setting_help")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(title);
                ui.label(text);
                if ui.button("Entendido").clicked() {
                    self.help = None;
                }
            });
            if response.should_close() {
                self.help = None;
            }
        }
    }
}
impl eframe::App for NipApp {
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let stroke = Stroke::new(1.7, if selected { ACCENT } else { TEXT });
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
    fn keyboard_chat_and_disconnect_do_not_start_capture() {
        let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let mut app = NipApp::new(commands, receiver);
        let ctx = egui::Context::default();
        configure(&ctx);
        events
            .send(Event::State(AppState {
                room: Some(Room {
                    id: "r".into(),
                    name: "Sala de prueba".into(),
                    self_id: 1,
                    members: vec![],
                    history: vec![],
                }),
                ..Default::default()
            }))
            .unwrap();
        let output = ctx.run_ui(Default::default(), |ui| app.render(ui));
        assert!(!output.shapes.is_empty());
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
        let _ = ctx.run_ui(input, |ui| app.render(ui));
        assert!(matches!(received.try_recv(), Ok(Command::Chat(text)) if text == "Hola al grupo"));
        events
            .send(Event::Disconnected("Servidor cerrado".into()))
            .unwrap();
        let _ = ctx.run_ui(Default::default(), |ui| app.render(ui));
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
            matches!(received.try_recv(), Ok(Command::Chat(text)) if text.chars().count() == 2000)
        );
        assert!(app.draft.is_empty());
    }
}
