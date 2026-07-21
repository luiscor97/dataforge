# SBOM — Software Bill of Materials

`dataforge.cdx.json` es el inventario completo de dependencias del
workspace en formato **CycloneDX 1.5**, con un componente por cada crate
(786: los 25 del workspace más las dependencias transitivas ancladas por
`Cargo.lock`), su versión, licencia SPDX y, para las de registro, su
Package URL `pkg:cargo/<nombre>@<versión>`.

## Generación

```bash
python scripts/generate-sbom.py > docs/sbom/dataforge.cdx.json
```

El generador solo necesita `cargo` y Python 3 — ningún subcomando extra de
cargo. Es **determinista y reproducible**: ordena los componentes y no
embebe timestamp, así que re-ejecutarlo contra el mismo `Cargo.lock`
produce un archivo byte-idéntico. Regenéralo (y revisa el diff) cada vez
que cambien las dependencias.

## Relación con las auditorías

El SBOM enumera; `cargo audit` y `cargo deny` (job "Dependency audit" de la
CI) juzgan. Juntos cubren la cadena de suministro: el SBOM dice *qué* se
enlaza, las auditorías dicen *si* algo es vulnerable, no está mantenido o
tiene una licencia no permitida. Las excepciones vigentes están anotadas y
fechadas en `deny.toml`.

## Firma

La firma del SBOM y de los artefactos de release es un acto de publicación
que requiere infraestructura de claves y autorización explícita; queda como
paso del proceso de release, no del build local.
