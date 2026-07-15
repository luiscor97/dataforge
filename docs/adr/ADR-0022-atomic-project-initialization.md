# ADR-0022 — Creación atómica de proyectos y marker endurecido

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 §35, §36, reglas 2 y 5; ADR-0002; threat model
T8; encargo P1-2 y P1-3

## Contexto

**P1-2.** `create_project` construía el proyecto *in situ*: creaba `state/`,
abría SQLite (aplicando migraciones), insertaba el proyecto y **por último**
escribía el marker. Un fallo en medio dejaba un directorio con base de datos y
sin marker: no abrible como proyecto y —peor— **ya no vacío**, así que la
validación rechazaba el reintento. El usuario quedaba atrapado con una carpeta
que DataForge no reconocía ni permitía reutilizar.

**P1-3.** El marker es un JSON corriente que cualquiera puede editar, y
`open_db` hacía `project_dir.join(&marker.database_path)`. En Windows,
**`join` con una ruta absoluta descarta la base**: un marker con
`C:\otro.sqlite` apuntaba el motor a otra base. Y `Connection::open` **crea**
el archivo si no existe, así que abrir un proyecto cuya base se hubiera perdido
devolvía silenciosamente un proyecto vacío en lugar de avisar.

## Decisión

### Creación atómica (P1-2)

1. **Staging + rename.** Todo se construye en `<project_dir>.init-<uuid>`,
   hermano del destino final para que el rename sea del mismo volumen y por
   tanto atómico:

   ```text
   crear estructura → SQLite → migraciones → integrity check
   → marker (escrito y sync_all) → cerrar la base → rename a project_dir
   ```

2. **El marker se escribe el último y solo tras el integrity check.** Es lo que
   convierte un directorio en proyecto: no puede aparecer sobre una base que no
   se ha demostrado sana. Se escribe con `sync_all` antes del rename, porque un
   marker presente pero vacío sería peor que ninguno.

3. **La base se cierra antes de finalizar.** Windows no renombra un directorio
   con handles abiertos dentro; el `Db` de staging se suelta explícitamente y el
   proyecto se reabre desde su ubicación definitiva.

4. **Solo se limpia lo que hemos creado nosotros.** Ante cualquier fallo se hace
   `remove_dir_all` **del staging**, jamás del directorio del usuario. Si
   `project_dir` ya existía (vacío, según la validación), se usa `remove_dir`
   —**nunca** `remove_dir_all`—: el sistema operativo falla si el directorio no
   está vacío, así que ni un error nuestro puede destruir datos. Antes se
   comprueba que no sea un reparse point, porque sobre un enlace "vacío" no
   significa nada.

### Marker endurecido (P1-3)

5. **`database_path` deja de ser autoritativo.** Se conserva por
   compatibilidad, pero debe ser **exactamente** la constante
   `state/dataforge.sqlite`; cualquier otra cosa (`..`, ruta absoluta, UNC,
   separador alternativo, otro nombre) se rechaza. Internamente se usa siempre
   la constante, nunca el campo.

6. **Política de versiones explícita.** Se compara el *major* de
   `schema_version` con el nuestro:
   - igual → compatible (minor/patch son aditivos);
   - mayor → **rechazo** con mensaje claro y remedio ("actualiza DataForge"),
     porque lo escribió una versión que sabe cosas que esta no;
   - menor → rechazo: necesitaría migración y no existe ninguna.

7. **Todo el marker se valida**: `schema`, `schema_version`, `project_id` (UUID
   válido **y** coincidente con la base) y `generator_version` no vacío.

8. **Abrir nunca crea.** Se comprueba que el archivo existe antes de abrirlo,
   porque `Connection::open` lo crearía.

## Alternativas consideradas

- **Escribir solo el marker de forma atómica y dejar el resto in situ** —
  descartada: resuelve "no abrible" pero no "no vacío", así que el reintento
  seguiría bloqueado.
- **Limpiar el directorio del usuario si la creación falla** — descartada de
  plano: el encargo lo prohíbe y es exactamente la clase de "conveniencia" que
  destruye datos.
- **Construir el staging *dentro* de `project_dir`** — descartada: el rename
  final dejaría de ser atómico y volveríamos al estado a medias.
- **Eliminar `database_path` del marker** (opción preferida del encargo) —
  aplazada: quitarlo rompería la lectura de markers 1.0.0 ya escritos. Se
  mantiene el campo pero **sin autoridad** y validado contra la constante, que
  da la misma garantía sin romper compatibilidad. Se eliminará cuando el schema
  del marker suba a 2.0.0.
- **Aceptar separadores alternativos (`state\dataforge.sqlite`)** — descartada:
  "sin separadores ambiguos" (encargo P1-3); una comparación exacta no admite
  interpretación.

## Consecuencias

- Un fallo durante la creación no deja rastro y el reintento funciona.
- Un marker manipulado no puede redirigir el motor a otra base ni fingir una
  versión.
- Coste: un rename extra por creación. Irrelevante.

## Limitaciones

- La atomicidad del rename depende del filesystem: NTFS local la ofrece; un
  recurso de red puede no hacerlo. NAS/UNC sigue experimental.
- El staging es hermano del destino, así que necesita permiso de escritura en
  el directorio padre. Si el usuario solo puede escribir dentro de una carpeta
  ya creada por él, la creación fallará; es un caso raro y preferimos fallar a
  perder la atomicidad.
- No hay migración de markers 1.0.0 → futuros; se definirá cuando exista un
  2.0.0.

## Compatibilidad

- Sin cambios de esquema SQLite.
- Los proyectos creados con v0.1.0 se siguen abriendo: su marker ya lleva
  `database_path = "state/dataforge.sqlite"` y `schema_version = "1.0.0"`.
- Sin cambios en el contrato de `df-facade` hacia CLI/UI.

## Tests

- `a_failed_creation_leaves_no_trace_and_retrying_works` — fuerza un fallo real
  tras la validación (el padre del proyecto es un archivo), comprueba que no
  queda medio proyecto ni staging huérfano, y que un reintento válido funciona.
- `the_marker_and_the_database_appear_together`.
- `a_preexisting_non_empty_directory_is_never_cleaned` — el archivo del usuario
  sigue byte a byte tras un intento fallido.
- `a_marker_cannot_redirect_the_database_outside_the_project` — 9 intentos:
  `../../otro.sqlite`, `..\..\otro.sqlite`, ruta absoluta, `/etc/passwd`,
  verbatim `\\?\`, UNC, separador alternativo, otro nombre, vacío.
- `a_future_marker_version_is_rejected_clearly` — 2.0.0 rechazado con remedio;
  1.9.3 aceptado; basura rechazada.
- `opening_never_creates_a_missing_database`.
- `a_marker_with_a_non_uuid_project_id_is_rejected`,
  `marker_and_database_must_agree_on_project_id`.
