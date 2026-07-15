# ADR-0018 — Manifiesto de ejecución inmutable

**Estado:** Aceptada
**Fecha:** 2026-07-15
**Relacionada con:** RFC-0001 regla 10, §26.3, §26.4, §27.1, §28;
ADR-0016, ADR-0019, ADR-0020; threat model T5

## Contexto

La regla 10 del RFC dice: *"toda ejecución parte de un plan aprobado e
inmutable"*. Hasta v0.1.1 eso era **medio verdad**, y la mitad que faltaba era
la que importaba.

El SHA-256 de aprobación cubría:

```text
sequence, operation_type, source_occurrence (id), content_id (id),
destination, idempotency_key
```

Pero el executor no ejecutaba eso. Resolvía el material real **en tiempo de
ejecución**, con joins vivos:

```sql
LEFT JOIN path_occurrences o ON o.id = p.source_occurrence
LEFT JOIN source_roots      r ON r.id = o.source_root_id
LEFT JOIN content_objects   c ON c.id = p.content_id
```

De ahí salían la ruta del origen, el fingerprint, el tamaño y los hashes
esperados. Consecuencia: **editar `content_objects.sha256` después de aprobar
cambiaba lo que se ejecutaba sin mover el hash del plan**. Lo mismo repuntando
`source_roots.absolute_path`. La aprobación firmaba los identificadores —el
papeleo— mientras el trabajo real quedaba atado a tablas mutables.

No hace falta un atacante sofisticado: basta cualquier proceso o persona con
acceso al `.sqlite` del proyecto entre `approve` y `execute` (actor A3 del
threat model).

## Decisión

1. **Un manifiesto congelado, y el executor no lee otra cosa.** La aprobación
   materializa una fila por operación en `execution_manifest` (migración
   `0004_execution_manifest`, sin tocar `0001`–`0003`) con **todo** lo que
   determina la ejecución:

   ```text
   qué se leerá:      source_root_id, source_root_identity,
                      source_root_path_snapshot, source_relative_path_exact,
                      source_raw_relative_path (ADR-0020), source_fingerprint
   qué se espera:     expected_size_bytes, expected_sha256, expected_blake3
   dónde se escribirá: destination_relative_path
   qué se ejecuta:    operation_type, operation_id, plan_id, sequence,
                      idempotency_key
   ```

2. **El hash de aprobación cubre el manifiesto entero**, no los identificadores.
   `serialize_manifest` → JSON canónico → SHA-256. Cambiar cualquier campo que
   decida qué se lee, qué se espera, dónde se escribe o qué se hace mueve el
   hash y la verificación lo detecta (`PLAN_TAMPERED`).

3. **Congelado dentro de la transacción de aprobación.** `approve_plan` inserta
   el manifiesto, escribe el hash y cambia los estados en **una sola**
   transacción: un plan no puede quedar `APPROVED` sin el manifiesto que define
   qué significa "aprobado".

4. **Inmutabilidad impuesta por la base de datos, no por buena conducta.**
   Triggers `execution_manifest_no_update` y `execution_manifest_no_delete`
   abortan cualquier `UPDATE` o `DELETE`. Un re-plan genera operaciones nuevas
   y, por tanto, filas nuevas: nada legítimo necesita mutar una.

5. **Las tablas de inventario vuelven a ser evidencia.**
   `executable_operations` lee solo de `execution_manifest`, uniéndose a
   `plan_operations` **exclusivamente** para las columnas mutables de progreso
   (`approval`, `execution_state`). Se pueden seguir usando para comprobaciones
   de consistencia; ya no son un contrato mutable.

6. **La identidad física del root se congela también** (`source_root_identity`,
   ADR-0019): no basta con la ruta, porque una ruta puede repuntarse.

## Alternativas consideradas

- **Añadir los campos a `plan_operations` y ampliar el hash** — descartada: esa
  tabla ya tiene columnas mutables de progreso (`execution_state`), así que un
  trigger de inmutabilidad total la rompería, y uno parcial es exactamente la
  clase de matiz que se cuela en una revisión. Separar contrato (inmutable) de
  progreso (mutable) hace la regla trivial de verificar.
- **Firmar el manifiesto con Ed25519** — descartada por ahora: sube el listón
  frente a quien edite la base, pero exige gestión de claves (§29.4 lo aplaza a
  una fase posterior). El hash ya da **detección**, que es la garantía que
  honestamente podemos ofrecer contra alguien con acceso al fichero.
- **Recalcular el manifiesto al ejecutar y comparar** — descartada: si se
  recalcula desde las tablas vivas, un cambio en ellas cambiaría ambos lados y
  la comparación pasaría. Hay que **almacenar** lo aprobado.
- **Exportar el manifiesto a JSON y ejecutar desde el archivo** — descartada:
  SQLite es la única fuente de verdad transaccional (regla 5). El JSON canónico
  existe como exportación auditable, no como contrato.

## Consecuencias

- "Aprobado" vuelve a significar algo verificable: lo ejecutado es exactamente
  lo firmado.
- La verificación re-hashea el manifiesto, así que una manipulación offline se
  detecta criptográficamente aunque el atacante tire los triggers.
- Coste: una fila extra por operación y una serialización canónica en la
  aprobación. Irrelevante frente a copiar bytes.
- Deuda: añadir un campo a `ManifestEntry` sin añadirlo al valor canónico
  reabriría el agujero en silencio. Hay un test de paridad struct/canonical que
  **falla el build** si eso ocurre; es intencionado y no debe relajarse.

## Limitaciones

- La garantía es de **detección**, no de prevención: quien puede editar la base
  también puede borrarla. Contra eso ninguna comprobación en proceso sirve, y
  el threat model lo dice explícitamente.
- El manifiesto congela la ruta y la identidad del root, pero el contenido del
  origen puede cambiar entre aprobación y ejecución: eso lo cubre el fingerprint
  (ADR-0019) y el re-hash al copiar, no este ADR.

## Compatibilidad

- Migración nueva (`0004`); `0001`–`0003` intactas.
- Los planes aprobados con v0.1.0 no tienen manifiesto. No se migran: un plan
  aprobado bajo el contrato viejo no puede reclamar la garantía nueva. Re-planear
  y re-aprobar es barato y honesto; inventar un manifiesto retroactivo a partir
  de tablas que ya pudieron ser editadas sería precisamente la mentira que este
  ADR elimina.
- Sin cambios en el contrato de `df-facade` hacia CLI/UI.

## Tests

- `editing_content_objects_after_approval_does_not_change_execution` — falsifica
  el sha256 vivo tras aprobar; el run no se inmuta (6 completadas, 0 fallidas,
  bytes correctos).
- `repointing_a_source_root_after_approval_does_not_change_execution` — repunta
  la raíz a un árbol señuelo; no redirige ni una lectura.
- `tampering_the_execution_manifest_fails_verification` — manipula el manifiesto
  tirando antes el trigger (como haría alguien con el `.sqlite`); veredicto
  `FAILED` con `PLAN_TAMPERED`.
- `the_execution_manifest_rejects_update_and_delete` — los triggers rechazan las
  vías normales.
- `the_canonical_value_covers_every_execution_field` — paridad struct/canonical.
- `approving_a_plan_freezes_it_under_a_canonical_hash` — el manifiesto
  almacenado re-hashea al hash de aprobación (reproducibilidad).
