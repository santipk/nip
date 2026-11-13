# Rust nativo y protocolo QUIC

Estado: implementación autorizada por el usuario. UI egui/eframe, backendCPAL/WASAPI yOpus, Tokio/quinn/rustls. Sin Chromium niWebView. Elegimosbibliotecasexistentes con licenciascompatibles con MIT, no un toolkit/codec/criptografíapropios. Icedfueconsiderado pero su upstream lo declaraexperimental; Slint requiere atenderlicenciasalternativas antesdeusarlo. Egui permiteventanasycontrolesnativosrenderizados, no navegador; mantieneaccesibilidadvíaAccessKit. No es una promesa de ahorro: memoriaTOTAL/CPU/arranque se medirá sobre ejecucionescontroladas.

AudioOpusmono48k20ms yrelayanfitrión víaQUIC/TLS1.3. EvitaimplementarICE/DTLS/SRTP de un WebRTCnativo en esta primera migración. ElanfitriónveaudioOpus ypuedeescucharlo: cifradoporsalto, no E2EE. UDPdebeanllegaralanfitrión; noSTUN/TURN/nattraversal/relaypúblico. Retransmisióndegrupo genera tráficoafanfitrión; no es unaarquitecturaparasalasgrandes. Invitacionesversionadas ynocompatiblesconElectron.

Certificadosgeneradosporrcgen, confiadossólo alcertificadodelainvitaciónmedianteRootCertStore yvalidaciónnip.local; permisorandom256bits. Authantesdechat/audio. PersistenciaseparadaconDPAPI Windows. No sepresuponenimportaciones deElectron ni seborraperfilviejo.

La paletaexistente espreferenciaexplícita; Impeccableseusa pararefinarla jerarquía/espaciado, no cambiarcolores. Lafuncionalidaddechat/pruebaprivada/ajustes/barra deaudio esrequisito, no simulaciónvisual.

Fuentes: https://github.com/emilk/egui (MIT/Apache-2.0); https://github.com/RustAudio/cpal (Apache-2.0); https://github.com/SpaceManiac/opus-rs (MIT/Apache-2.0, libopusBSD); https://github.com/quinn-rs/quinn (MIT/Apache-2.0); https://github.com/rustls/rustls (Apache-2.0/ISC/MIT). Laslicenciasdetransitivasseconservaránenladistribución.
