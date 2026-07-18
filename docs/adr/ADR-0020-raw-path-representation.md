# ADR-0020 — Representación raw de rutas

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 §13.4, §9.3; ADR-0018; threat model T9

## Contexto

Las rutas de Windows son secuencias de **unidades de código UTF-16**, y no
tienen por qué ser UTF-16 válido: un surrogate suelto (p. ej. `0xD800` sin su
pareja) es un nombre de archivo perfectamente legal, y aparece en material
recuperado, en discos de otros sistemas y en archivos creados por software
antiguo.

Hasta v0.1.1 el escáner guardaba el nombre con `to_string_lossy` y marcaba
`name_is_lossy = 1`. Ese flag decía *que* el nombre estaba dañado, pero no
*cuál era*: no sirve de nada cuando el trabajo consiste en copiar ese archivo.

Y es peor que "no sirve": `to_string_lossy` sustituye lo irrepresentable por
U+FFFD, que **es un carácter real**. Reabrir con el nombre lossy puede fallar
o, en el peor caso, abrir **otro archivo** que sí se llame así. Un motor cuya
promesa es "copio exactamente lo que inventarié" no puede permitirse eso
(amenaza T9).

## Decisión

1. **Tres formas, nunca confundidas.**

   | forma | qué es | para qué |
   |---|---|---|
   | **display** | `to_string_lossy` | enseñar a un humano, logs, informes |
   | **comparison** | display en minúsculas | agrupar, claves de dedup, índices |
   | **raw** | las unidades UTF-16 exactas | **abrir el archivo** |

   Solo la raw es autoritativa. Usar la display para tocar el sistema de
   archivos es un bug, no un atajo.

2. **Una sola estrategia de almacenamiento: UTF-16 little-endian en BLOB.**
   El encargo admitía BLOB, array JSON de u16 o hex/base64. Se elige **BLOB
   UTF-16LE** en SQLite (`path_occurrences.raw_relative_path`,
   `folders.raw_relative_path`, `execution_manifest.source_raw_relative_path`,
   migración `0005_path_identity`). Donde un blob no puede viajar —el JSON
   canónico del manifiesto— se renderizan **los mismos bytes** en hex
   minúscula. Una estrategia, una codificación, dos renderizados.

3. **El tipo `RawPath` encapsula la conversión.** `from_os_str` /
   `to_os_string` usan `OsStrExt::encode_wide` / `OsStringExt::from_wide`, que
   son exactos y no pasan por `String`. `display()` y `comparison_key()`
   existen, pero están claramente marcados como no autoritativos.

4. **El manifiesto aprobado lleva la ruta raw**, y está cubierta por el hash de
   aprobación (ADR-0018): repuntar la ruta raw tras aprobar rompe la
   verificación igual que cualquier otro campo. El test de paridad
   struct/canonical obliga a ello.

5. **El executor reabre desde la raw.** `source_path()` reconstruye la ruta
   desde `source_raw_relative_path`; la display solo se usa como *fallback*
   para snapshots anteriores a v0.1.1, que no tienen forma raw. Ese fallback
   está acotado y comentado.

6. **Nullable, sin inventar.** Las columnas son nulables porque los snapshots
   antiguos no tienen raw. `NULL` significa "solo display, degradado", no se
   fabrica una raw a partir de la display —sería justo el error que este ADR
   evita.

## Alternativas consideradas

- **Array JSON de u16** — descartada: legible pero triplica el tamaño en la
  base y obliga a parsear JSON para abrir un archivo.
- **Base64** — descartada frente a hex: hex es trivialmente inspeccionable en
  un dump y el manifiesto no es un formato de gran volumen.
- **Guardar solo la ruta absoluta raw** — descartada: duplicaría la raíz en
  cada fila y desincronizaría el inventario si la raíz se reubica. Se guarda
  la **relativa** raw, igual que la display.
- **Normalizar los nombres irrepresentables al escanear** (renombrar, escapar)
  — descartada de plano: modificaría el origen, contra la regla 1.
- **Seguir con `name_is_lossy` y avisar al usuario** — descartada: es
  precisamente el estado actual, y no permite copiar el archivo.

## Consecuencias

- Un nombre no representable se inventaría, se hashea y se copia sin pérdida.
- La UI sigue mostrando texto legible; nada cambia para el usuario normal.
- Coste: dos bytes por unidad de código por ocurrencia. Despreciable.
- Cualquier código nuevo que abra un origen debe usar la raw. Es la clase de
  regla que hay que vigilar en revisión.

## Limitaciones

- **Solo Windows** tiene captura exacta (`encode_wide`). En otras plataformas
  `from_os_str` pasa por `to_string_lossy`: los nombres que no son UTF-8
  válido son un problema de Unix y quedan **fuera de alcance** en v0.1.1-dev.
  No se anuncia lo contrario.
- El *destino* en la salida sigue siendo una cadena validada
  (`SafeRelativePath`): DataForge elige los nombres de salida, así que no
  hereda nombres irrepresentables. Si algún día un perfil los propaga, habrá
  que extender esto.
- `comparison_key()` usa la display: dos nombres raw distintos pueden compartir
  clave. Es correcto para agrupar y **fatal para abrir**; por eso son tipos y
  métodos distintos.

## Compatibilidad

- Migración `0005_path_identity` solo añade columnas nulables (`ALTER TABLE`);
  `0001`–`0003` intactas.
- Un snapshot anterior a v0.1.1 se sigue abriendo: sin raw, el executor usa la
  display como antes (degradado, documentado).
- Sin cambios en el contrato de `df-facade`.

## Tests

- `ordinary_unicode_names_round_trip` — ñ, cirílico, CJK, emoji, vacío.
- `an_unpaired_surrogate_survives_the_round_trip_exactly` — el caso que motiva
  todo: comprueba que la display **se daña** (`a\u{FFFD}b`), que reencodear la
  display **no recupera** el original (la prueba de por qué la lossy no puede
  tocar el disco) y que la raw sí sobrevive por blob y por hex.
- `the_blob_is_little_endian_utf16` — fija el formato exacto.
- `malformed_stored_values_are_rejected` — longitud impar, hex inválido.
- `the_comparison_key_is_separate_from_the_raw_form`.
- `a_real_unicode_file_reopens_through_its_raw_path` (Windows, archivo real).
- `sources_are_reopened_through_their_raw_path` (**integración de extremo a
  extremo**): escanea, hashea, planifica, aprueba y ejecuta un archivo llamado
  `acta ñ 文件 🎉.txt`, comprobando que el manifiesto congela la raw y que los
  bytes llegan a la salida.
