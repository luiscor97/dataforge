# ADR-0021 — Finalize no-replace por plataforma y durabilidad

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 regla 3, §27.1, §27.3; ADR-0017; threat model
T3, T4, T8

## Contexto

La regla 3 dice que DataForge no sobrescribe. Hasta v0.1.0 el executor lo
implementaba así:

```rust
if destination.exists() { return Err(DestinationChanged); }
std::fs::rename(&partial, &destination)?;   // "falla si el destino apareció"
```

Ese comentario es **falso en Windows**. `std::fs::rename` llama a `MoveFileExW`
con `MOVEFILE_REPLACE_EXISTING`, así que si el destino aparece entre el
`exists()` y el `rename`, **lo sobrescribe en silencio**. El `exists()` es una
comprobación TOCTOU: no es una garantía, es una carrera que casi siempre gana
DataForge y ocasionalmente pierde el usuario, perdiendo un archivo.

Es exactamente el patrón que el encargo prohíbe: creer implementada una
protección porque hay una comprobación previa.

## Decisión

1. **La garantía la da el kernel.** `df_fs_safety::finalize_no_replace` llama a
   `MoveFileExW` **sin** `MOVEFILE_REPLACE_EXISTING`. Si el destino existe, la
   propia llamada falla con `ERROR_ALREADY_EXISTS`, que se traduce al error
   tipado `DestinationExists` → `OperationErrorCode::DestinationChanged`. No
   hay ventana: no existe un instante en que el destino exista y aun así lo
   escribamos.

2. **`std::fs::rename` queda prohibido para finalizar.** Está documentado en el
   propio crate. El `exists()` previo puede mantenerse como *atajo* para fallar
   antes y barato, pero ya no es la garantía.

3. **Durabilidad, con matices.** La secuencia es:

   ```text
   escribir parcial → flush + sync_all(parcial) → finalize_no_replace → registrar resultado en SQLite (transaccional)
   ```

   `MoveFileExW` recibe `MOVEFILE_WRITE_THROUGH`, que en NTFS pide que el
   cambio de metadatos esté en disco antes de retornar.

4. **Qué NO se promete.** No se usa el término "durabilidad garantizada".
   `sync_all` + `MOVEFILE_WRITE_THROUGH` cubren el camino normal (caída del
   proceso, kill, cierre de sesión). **No** cubren un fallo físico del
   dispositivo, una caché de disco que miente sobre el flush, ni un filesystem
   de red que reordena. Ante eso la reanudación y el verificador son la red de
   seguridad: un parcial huérfano se detecta y una copia sin resultado en la
   base se re-ejecuta.

5. **Plataformas sin primitiva segura: bloquear.** En no-Windows,
   `finalize_no_replace` devuelve `UnsupportedPlatform` y la ejecución no
   arranca. Cuando se implemente Unix, la primitiva será `renameat2` con
   `RENAME_NOREPLACE` (Linux ≥3.15) o `link()`+`unlink()` como fallback
   documentado, nunca un `exists()`+`rename`.

## Alternativas consideradas

- **Mantener `exists()` + `rename`** — descartada: es la vulnerabilidad, no la
  solución.
- **`CreateFileW` con `CREATE_NEW` sobre el destino y copiar dentro** —
  descartada: da la garantía de no-replace pero pierde la atomicidad del
  destino (un lector podría ver el archivo a medias) y complica la limpieza.
  El patrón parcial + rename atómico es mejor.
- **`ReplaceFileW`** — descartada: su propósito es exactamente reemplazar.
- **`SetFileInformationByHandle` con `FILE_RENAME_INFO` y
  `ReplaceIfExists=FALSE`** — equivalente y más moderno; se prefirió
  `MoveFileExW` por simplicidad y por estar disponible en toda la gama Windows
  soportada. Revisable sin cambiar el contrato del crate.

## Consecuencias

- Desaparece la ventana conocida de sobrescritura silenciosa.
- Una colisión de destino ahora falla siempre de forma tipada y auditable, en
  vez de depender de quién gane la carrera.
- El coste es nulo: es la misma syscall con un flag menos.

## Limitaciones

- Solo Windows en v0.1.1-dev.
- La atomicidad del rename depende del filesystem; NTFS local la ofrece, un
  recurso de red puede no hacerlo. NAS/UNC sigue experimental.
- No hay garantía ante fallo físico del hardware (punto 4).

## Compatibilidad

- Sin cambios de esquema ni de formato.
- El código de error observable por CLI/UI sigue siendo
  `DESTINATION_CHANGED`, así que no cambia el contrato de `df-facade`.

## Tests

- `finalize_no_replace_refuses_an_existing_destination` — comprueba el error
  tipado **y** que el archivo preexistente conserva su contenido original y el
  parcial sigue ahí (no se pierde nada).
- `finalize_no_replace_moves_when_the_destination_is_free` — el camino feliz
  sigue funcionando.
- `create_partial_secure_never_reuses_an_existing_file`.
- Pendiente en el commit del executor: el test de integración
  "el destino aparece justo antes del finalize" (T3), que es el que demuestra
  el fin de la carrera de extremo a extremo.
