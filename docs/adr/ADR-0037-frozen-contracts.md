# ADR-0037 — Contratos congelados y su test de regresión (M0.9)

**Estado:** Aceptada
**Fecha:** 2026-07-20
**Relacionada con:** RFC-0001 §45 M0.9; ADR-0007, ADR-0019, ADR-0026, ADR-0030..0034

## Contexto

M0.9 exige congelar contratos, schema, API y ABI antes de 1.0. Hasta ahora
esas versiones vivían dispersas como constantes en cada crate
(`ANALYSIS_CONTRACT_VERSION`, `HOST_ABI_VERSION`, los ids de schema JSON,
la versión del perfil, el marcador, las familias de algoritmo…) y la cadena
de migraciones. Nada impedía que un cambio accidental de valor pasara la
puerta de calidad: la evidencia histórica (runs sellados, DBs de usuario,
plugins firmados, auditorías de IA) depende de que esos identificadores no
muten en silencio.

## Decisiones

1. **Un inventario único y ejecutable.** El test
   `df-facade::frozen_contracts` fija en un solo lugar cada versión de
   schema, algoritmo y ABI, el número y orden de las migraciones (1–19
   consecutivas, `foundation`…`incremental_reuse`) y falla si cualquiera
   cambia. df-facade es el punto correcto: depende de todos los crates de
   dominio y ve el `MARKER_SCHEMA_VERSION` interno.

2. **Congelar es subir versión + ADR, nunca editar in place.** Si el test
   se rompe, o es un bump deliberado (se actualiza la expectativa en el
   mismo commit que el ADR que lo justifica) o es un accidente que revertir.
   Esto codifica la regla que ya seguían las migraciones (checksum drift)
   para *todos* los contratos.

3. **Exposición mínima de los identificadores de contrato.** Se hacen
   públicos `PROFILE_SCHEMA`/`PROFILE_SCHEMA_VERSION` (df-domain) y
   `REQUEST_SCHEMA_VERSION` (df-ai, hermano de los ya públicos), porque un
   identificador de contrato congelado es legítimamente parte de la
   superficie pública. No se expone nada operativo interno.

## Inventario congelado (a fecha de esta ADR)

- Migraciones: 0001–0019, checksummed, append-only.
- Perfil: `dataforge.profile` v1.1.0.
- Marcador de proyecto: schema 1.0.0.
- Similaridad: `fastcdc-v2020-l1-minhash-v1` (fastcdc =3.2.1).
- Contenido: extractor `0.2.0+content-v1`, `m0.4-tantivy-v1`,
  `m0.4-parquet-v1`.
- Media: `dataforge.media-analysis.v1`; `dct-phash64-v1`,
  `rusty-chromaprint-0.3.0-test2-v1`, `sampled-dct-phash64-v1`.
- Plugin ABI: `0.1.0`; manifest/input/findings `…/0.1.0`.
- IA: request/disclosure/audit/suggestions `…/0.7.0`, prompt `…/0.7.0`.

## Alternativas consideradas

- **Un test de freeze por crate** — descartado: correcto pero disperso; un
  inventario único es auditable de un vistazo y es lo que un revisor de
  release necesita.
- **Solo documentar los valores en un .md** — descartado: un documento no
  falla la CI cuando alguien cambia una constante.

## Consecuencias

- Cualquier deriva de contrato rompe la puerta, no un usuario en producción.
- El extractor incluye la versión del paquete (`CARGO_PKG_VERSION`), así que
  subir la versión del workspace cambia su identidad de forma consciente y
  visible; el freeze lo hace explícito.
- Deuda: el freeze cubre versiones e identificadores; la forma exacta de los
  schemas JSON se cubre con sus propios tests de esquema cerrado por crate.
