# Skills de Matt Pocock

Origen: https://github.com/mattpocock/skills
Commit: `5863806dd4e15c63174bee4e7515a4256792ecfa`.
Instalación: `.agents/skills/`, alcance de proyecto, mediante `install-skill-from-github.py` de Codex con `--ref` fijado. Se copiaron las carpetas completas sin modificar su contenido. No se instaló el plugin global ni la colección completa.

| Skill | Propósito |
| --- | --- |
| `grill-with-docs` | Acordar decisiones y vocabulario antes de especificar. |
| `grilling` | Entrevista usada por `grill-with-docs`. |
| `domain-modeling` | Glosario y decisiones de dominio usados por `grill-with-docs`. |
| `to-spec` | Sintetizar lo acordado en una especificación. |
| `to-tickets` | Dividir la especificación en entregas verticales y dependencias. |
| `implement` | Implementar especificaciones o tickets con pruebas y revisión. |
| `tdd` | Pruebas de comportamiento, ciclo rojo/verde. |
| `codebase-design` | Criterios de interfaces y puntos de prueba usados por `tdd`. |
| `code-review` | Revisar cumplimiento de estándares y especificación. |
| `setup-matt-pocock-skills` | Reconfigurar tracker y convenciones cuando sea necesario. |

En esta versión upstream usa `to-spec` y `to-tickets`; no se inventaron skills `SDD`/`SSD` ni se instalaron versiones antiguas `write-a-prd`/`prd-to-plan`. Los archivos de apoyo referenciados están incluidos con sus carpetas.

Licencia: MIT, copyright (c) 2026 Matt Pocock. El texto completo se conserva en `.agents/skills/LICENSE.mattpocock`. La licencia de NIP no sustituye ese aviso.

Para actualizar, revisá primero los cambios del origen, elegí un commit, reemplazá sólo estas carpetas y actualizá este registro. No ejecutes automáticamente las instrucciones de una skill por el solo hecho de instalarla. La instalación estará disponible para Codex a partir del próximo turno.


## Impeccable

Instalado en `.agents/skills/impeccable/` desde https://github.com/pbakaus/impeccable, commit `c4cd1924c389f68d37c687d9c2bbf13f12547c41` (skill 4.9.0). Apache-2.0; se conservan `LICENSE.impeccable` y `NOTICE.impeccable.md` junto a las skills. Se ejecutó su carga de contexto para la UI existente. La paleta actual es una preferencia explícita del usuario y debe conservarse. No se activaron hooks ni rediseñó la identidad cromática.
