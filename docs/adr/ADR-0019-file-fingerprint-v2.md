# ADR-0019 — Fingerprint físico v2

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 §13.5, §14.1, §14.5, §27.1; ADR-0017; threat
model T6

## Contexto

`FileFingerprint v1` era `(size_bytes, modified_at_fs)`. Sirve para detectar
que un archivo creció o se editó, y para eso ha funcionado. Pero no cubre el
caso que más importa en material heredado: **sustituir un archivo por otro
distinto del mismo tamaño y con la misma fecha de modificación**. Preservar el
mtime no requiere malicia sofisticada — lo hace cualquier herramienta de copia
con `/COPY:T`, y `SetFileTime` lo hace en tres líneas.

Con v1, DataForge podía hashear un archivo y copiar otro, y llamar verificado
al resultado. El fingerprint prometía una identidad que no tenía.

Además, todo el pipeline comparaba fingerprints **como strings**
(`pre != job.fingerprint`), sin parsear: el dominio no sabía qué estaba
comparando, y un token de otra versión habría dado "distinto" o "igual" por
accidente de formato, no por semántica.

## Decisión

1. **Enum versionado, no struct.**

   ```rust
   pub enum FileFingerprint { V1(FingerprintV1), V2(FingerprintV2) }
   ```

   `v1` se conserva **solo para leer** tokens escritos por versiones
   anteriores. Nada genera v1 nuevos.

2. **v2 incluye identidad física.** En Windows, vía
   `GetFileInformationByHandle` sobre un handle abierto con
   `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` (así no se puede
   engañar para que huella el destino de un enlace):

   ```text
   size_bytes
   modified_at_ms
   change_time_ms   (GetFileInformationByHandleEx / FILE_BASIC_INFO)
   attributes
   identity = (volume_serial, file_id)
   ```

   Dos archivos distintos no pueden compartir `file_id` en el mismo volumen,
   así que la sustitución se detecta. El `change_time` de NTFS es un extra
   valioso: se mueve al cambiar metadatos aunque el escritor restaure el mtime.

3. **Token versionado y parseado siempre.**

   ```text
   v1:<size>:<mtime|none>
   v2:<size>:<mtime|none>:<ctime|none>:<attrs>:<volume|none>:<file_id|none>
   ```

   El prefijo garantiza que un v1 nunca compare igual a un v2 por accidente. El
   dominio **parsea antes de comparar**; se retiró la comparación de strings.

4. **Veredicto explícito en vez de `PartialEq`.**

   ```rust
   pub enum FingerprintVerdict { SamePhysical, SameDegraded, Changed }
   ```

   Una igualdad booleana obligaría a responder sí/no y escondería la diferencia
   entre *"el mismo archivo, demostrado"* y *"no veo que haya cambiado"*. Esa
   distinción es justamente lo que v1 no podía hacer, así que el tipo la dice en
   voz alta. `compare()` no es `PartialEq` a propósito.

5. **Identidad degradada ≠ sin cambios.** Cuando el filesystem no da `file_id`
   (algunos redirectores de red), el fingerprint es v2 **degradado**:
   `guarantee()` devuelve `Degraded` y el mejor veredicto posible es
   `SameDegraded`. Un v1 almacenado es siempre degradado: no lleva identidad.
   **v1 y v2 no se declaran equivalentes**: comparar uno con otro solo puede dar
   `SameDegraded` o `Changed`, nunca `SamePhysical`.

6. **Compatibilidad sin migración de datos.** Los tokens viven en
   `path_occurrences.fingerprint` como TEXT; un snapshot antiguo sigue
   abriéndose y comparándose por lo que lleva. No hace falta migración: el
   parser acepta ambos.

## Alternativas consideradas

- **Añadir campos al struct v1 sin versionar** — descartada: los tokens ya
  almacenados dejarían de parsear o, peor, compararían mal en silencio.
- **Usar `PartialEq` sobre el enum** — descartada: `a == b` no puede expresar
  "coinciden en todo lo que ambos llevan, pero uno no tiene identidad". Esa
  ambigüedad es el bug de v1 reencarnado.
- **Hashear siempre el contenido en vez de un fingerprint** — descartada: el
  fingerprint existe precisamente para *evitar* releer bytes; el hash completo
  ya se hace en su fase (§14) y es lo que se compara al copiar.
- **`std::os::windows::fs::MetadataExt::file_index()`** — descartada: sigue
  siendo API inestable en stable Rust (`windows_by_handle`), así que hay que
  llamar a `GetFileInformationByHandle` vía `windows-sys`.
- **Incluir `creation_time` en vez de `change_time`** — descartada:
  `creation_time` se preserva en copias y no aporta señal de manipulación;
  `ChangeTime` es la que se mueve cuando alguien toca los metadatos.

## Consecuencias

- La sustitución con mismo tamaño y mismo mtime se detecta donde el filesystem
  ofrece file id — que es el caso normal en NTFS local.
- Un archivo **movido dentro del mismo volumen** conserva su `file_id`, así que
  el fingerprint lo reconoce como el mismo objeto. Es lo correcto: que la
  *ruta* haya cambiado es otra pregunta, y la responde la ocurrencia, no el
  fingerprint.
- Copiar el contenido a un archivo nuevo produce identidad distinta →
  `Changed`.
- Coste: una llamada extra (`GetFileInformationByHandleEx`) por captura.
  Despreciable frente al hash.

## Limitaciones

- Identidad física solo en Windows. En otras plataformas la captura degrada a
  size+mtime **etiquetado como degradado**, no fingido como fuerte.
- Sin `file_id` (NAS/UNC, algunos redirectores) la garantía baja a
  `SameDegraded`: no se puede descartar sustitución. NAS/UNC sigue
  experimental.
- `file_id` puede reutilizarse tras borrar un archivo en NTFS; combinado con
  size+mtime+ctime el falso "mismo archivo" es muy improbable, pero no
  imposible.

## Compatibilidad

- Tokens v1 existentes se siguen leyendo (test explícito).
- Sin cambio de esquema ni migración.
- Sin cambio en el contrato de `df-facade` hacia CLI/UI.

## Tests

- `v1_tokens_still_parse`, `tokens_round_trip_in_both_versions`,
  `malformed_tokens_are_rejected`.
- `a_substitution_with_identical_size_and_mtime_is_detected` (unidad) y
  `a_same_size_same_mtime_substitution_is_detected` (**integración con
  archivos reales**: sustituye el archivo y restaura el mtime con
  `set_modified`, comprobando primero que tamaño y mtime coinciden — si no, el
  test no probaría nada — y luego que el veredicto es `Changed`).
- `a_move_within_the_volume_keeps_the_identity`,
  `a_copy_of_the_content_is_a_different_object`.
- `without_identity_the_verdict_is_degraded_never_physical`,
  `v1_and_v2_never_compare_as_physically_same`.
- `a_captured_fingerprint_carries_physical_identity` (Windows).
- El escáner comprueba que produce v2 con identidad física en NTFS local.
