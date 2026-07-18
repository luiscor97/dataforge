# ADR-0017 — Frontera segura del sistema de archivos (`df-fs-safety`)

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 reglas 1/2/3, §27, §37; threat model
[`filesystem-hardening.md`](../threat-model/filesystem-hardening.md) (T1, T2,
T3, T4, T10)

## Contexto

Hasta v0.1.0 el executor validaba el destino de forma **textual**: relativo, sin
`..`. Después construía la ruta con `output_root.join(destino)` y escribía con
`create_dir_all`, `File::create` y `std::fs::rename`.

Esa validación no demuestra nada sobre el sistema de archivos. Si dentro de la
salida ya existe `Salida\clientes` y resulta ser una junction hacia
`C:\DatosExternos`, entonces `clientes\archivo.pdf` es un destino relativo
perfectamente válido… que escribe fuera de la salida autorizada. Es el ataque
T1 del threat model, y no requiere malicia: basta con material heredado.

`canonicalize` tampoco sirve: **sigue** los enlaces, así que devuelve la ruta
escapada como si fuera legítima. Es exactamente la herramienta equivocada.

Además, al revisar `std::fs::rename` para este ADR encontramos un problema peor
(T4): en Windows llama a `MoveFileExW` **con** `MOVEFILE_REPLACE_EXISTING`, es
decir, **sobrescribe**. La única protección era un `destination.exists()`
previo, que es una comprobación TOCTOU. El comentario del código afirmaba que
el rename fallaba si el destino aparecía: en Windows era falso. Eso es una
violación potencial de la regla 3.

## Decisión

Se crea el crate **`df-fs-safety`**: toda escritura de DataForge pasa por él.
Ningún otro crate puede usar `create_dir_all`, `File::create` ni `fs::rename`
sobre la salida.

Ofrece:

- `SafeRelativePath` — la mitad **textual** (relativo, sin `..`, sin raíz ni
  prefijo, sin componentes que Windows recorte). Explícitamente débil: no
  pretende demostrar nada del filesystem.
- `SafeOutputRoot` — la mitad **física**: valida el root, lo identifica por
  `(volume_serial, file_id)` vía `GetFileInformationByHandle` sobre un handle
  abierto con `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`, y
  puede **revalidar** esa identidad durante la ejecución.
- `SecureDestination` — un destino ya demostrado dentro de su root.
- `resolve_destination_without_following_links` / `inspect_path_components` —
  recorren el destino **componente a componente** rechazando cualquier
  componente existente con `FILE_ATTRIBUTE_REPARSE_POINT`. Ese atributo cubre
  symlink, junction y mount point por igual, que es justo lo que queremos
  rechazar sin distinguir.
- `create_directory_secure` — sustituye a `create_dir_all`: comprueba cada
  nivel y vuelve a comprobar tras crearlo, de modo que una carpeta que se
  convierte en junction a mitad se detecta.
- `create_partial_secure` — `create_new`, nunca reutiliza ni trunca un parcial.
- `finalize_no_replace` — ver ADR-0021.

Principio: **la garantía la da el sistema operativo, no una comprobación
previa**. Un `exists()` antes de escribir es una carrera; que el kernel
rechace la operación no lo es.

## Alternativas consideradas

- **Seguir usando `canonicalize` + comparación de prefijos** — descartada: sigue
  enlaces, así que valida precisamente el escape que queremos impedir.
- **Solo validación textual mejorada** (más patrones prohibidos) — descartada:
  el problema no está en el texto del destino sino en el estado del disco. El
  encargo lo prohíbe explícitamente ("no declares una protección implementada
  solo porque una ruta se valida textualmente").
- **Mantener handles abiertos de toda la cadena de directorios durante la
  ejecución** (la única defensa TOCTOU completa) — descartada por ahora: obliga
  a reescribir la E/S sobre APIs `*_by_handle` y a mantener descriptores
  abiertos durante copias largas. Se reduce el riesgo con revalidación de
  identidad + finalize no-replace, y se documenta el residual (T3).
- **Usar el crate `winapi` en vez de `windows-sys`** — descartada:
  `windows-sys` es el binding oficial de Microsoft, mantenido y con licencia
  MIT/Apache-2.0 compatible.

## Consecuencias

- El ataque de junction del threat model queda demostrablemente bloqueado, con
  un test que crea una junction real y comprueba además que **no se escribió
  nada fuera**.
- El coste es una llamada de metadatos por componente y por operación. Es
  despreciable frente al hash y la copia.
- Aparece una dependencia nueva (`windows-sys`), solo en target Windows.
- Cualquier código futuro que escriba en la salida debe pasar por este crate;
  saltárselo es una regresión de seguridad y debe rechazarse en revisión.

## Limitaciones

- **Solo Windows.** En cualquier otra plataforma `SafeOutputRoot::validate`
  devuelve `UnsupportedPlatform` y **bloquea la ejecución**, en lugar de fingir
  una garantía que no existe (RFC-0001 regla 19). Linux/macOS necesitarán
  `openat`/`O_NOFOLLOW`/`renameat2`, y hasta entonces no se anuncia soporte.
- **Ventana TOCTOU residual**: entre la validación de componentes y la
  escritura sigue existiendo un hueco. Se mitiga, no se elimina: el finalize
  no-replace convierte "pisar" en "fallar".
- **Identidad degradada**: si el filesystem no da `file_id` (algunos
  redirectores de red), `identity_level()` es `Degraded` y la revalidación no
  puede comparar. Se registra; no se presenta como identidad fuerte. NAS/UNC
  sigue siendo experimental.

## Compatibilidad

- No cambia el esquema SQLite ni ningún formato exportado.
- No cambia el contrato de `df-facade` hacia CLI/UI.
- Es aditivo: el executor pasa a usarlo en un commit posterior.

## Tests

- `relative_paths_reject_traversal_and_absolutes`,
  `trailing_dots_and_spaces_are_rejected`, `current_dir_components_are_ignored`
  — la capa textual.
- `the_output_root_has_a_physical_identity`,
  `identity_distinguishes_two_directories` — identidad real por handle.
- `a_reparse_point_root_is_rejected`,
  `a_junction_component_inside_the_output_is_rejected` — el ataque T1/T2 con
  una junction real creada con `mklink /J`, comprobando que el directorio
  externo queda vacío.
- `inspect_reports_components_without_following`.
- `create_partial_secure_never_reuses_an_existing_file`.
- Los tests que necesitan junctions se saltan **imprimiendo el motivo** si el
  entorno no las permite; nunca pasan en silencio.
