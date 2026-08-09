# ESTADO — empieza por aquí

Punto de entrada único del repositorio. **Siempre esta ruta, siempre este
nombre.** Si retomas DataForge desde cualquier equipo, o eres una sesión nueva
a la que le han dicho «mira el repo a ver los últimos cambios», lee esto
primero y sigue los enlaces desde aquí.

Existe porque el trabajo se reparte entre varios equipos que no comparten
memoria, y ocho documentos de estado compitiendo por ser el primero significa
que cada sesión lee uno distinto y ve una película distinta.

**Lo que git no trae no existe.** El código y los documentos viajan; una
prueba que nadie escribió, no. Ese es el único hueco real entre equipos, y se
cierra escribiendo, no sincronizando.

---

## 1. Dónde está cada cosa

| Necesitas | Ve a | |
| --- | --- | --- |
| Las reglas del proyecto | [RFC-0001](rfcs/RFC-0001-dataforge-foundation-and-roadmap.md) | |
| Qué es la fase agéntica | [RFC-0002](rfcs/RFC-0002-autonomy-ladder.md) | |
| Usar la herramienta | [manual](manual/README.md) | |
| Antes de una prueba real | [field-test-readiness](release/field-test-readiness.md) | |
| Qué pasó en pruebas reales | [field-runs](testing/field-runs/README.md) | |
| Garantía → evidencia de la 1.0 | [m1.0-acceptance](release/m1.0-acceptance.md) | |
| Foto completa del repo | `roadmap/estado-del-repo-2026-08-09.md` | ⧗ |
| Qué hacer a continuación | `roadmap/continuidad-fase-agentica-2026-08-09.md` | ⧗ |
| El destino de la 2.0 | `roadmap/ROADMAP-2.0.md` | ⧗ |
| Qué está implementado ya | `roadmap/estado-superficie-agentica.md` | ⧗ |
| Mediciones sobre el corpus real | `roadmap/hallazgos-2026-08-01.md` | ⧗ |
| Reanudar un trabajo a medias | `roadmap/reanudar-el-trabajo-en-curso.md` | ⧗ |

**⧗ = todavía no está en `main`.** Llega con la [PR #45](https://github.com/luiscor97/dataforge/pull/45),
que es lo primero de la lista de §3. Hasta entonces se leen desde la rama
`feat/agent-drivable-engine`. Van sin enlace a propósito: un índice que apunta
a un fichero que no existe es peor que uno que dice dónde está.

Los documentos con fecha en el nombre son **histórico**: valen por lo que
midieron el día que se escribieron. Este índice es lo único que se mantiene al
día.

## 2. Cómo empieza y cómo termina una sesión

**Al empezar**, siempre:

```bash
git fetch --all --prune
git log --oneline origin/main -5
gh pr list --state open
```

Y leer §3 de este documento. Antes de abrir rama, `git branch -r`: ya ha
pasado dos veces que dos líneas de trabajo hicieran lo mismo sin saberlo.

**Al terminar**, añadir una línea a §4. No es burocracia: es la única forma de
que el siguiente equipo sepa qué pasó aquí. Una sesión que no deja línea es una
sesión que el resto no puede ver.

Si la sesión ejecutó DataForge contra datos reales, además va un fichero en
[`testing/field-runs/`](testing/field-runs/README.md).

## 3. Estado actual

> Actualizar esta sección cuando cambie, no cada día.

**Versión.** `v1.0.0` está taggeada pero **nunca se publicó**; su borrador
lleva wasmtime 36.0.12 (RUSTSEC-2026-0222) y conviene retirarlo. La **1.0.1**
es la primera versión publicable y está en la PR #48 — que además repara que
`main` no pueda producir release (su SBOM está desincronizado del `Cargo.lock`).

**Milestone.** La 2.0, fase agéntica. RFC-0002 aprobada y en `main`. Todo el
trabajo vive en la PR #45. M2.1 y M2.2 tienen código; M2.3 está bloqueado por
decisiones; M2.4–M2.6 tienen núcleo pero **no están cableados**.

**Lo que se entrega a continuación no es la 2.0, es L1 supervisado**: el agente
observa y propone, el humano ocupa la puerta. La clase `commit` de `df-mcp`
está cerrada hasta que `df-rules` sea la autoridad.

**Esperando decisión humana:**

1. Mergear #48 y empujar `v1.0.1` (publicar es acto humano, ADR-0039).
2. Cerrar #45 antes de que crezca más.
3. Reencuadre de M2.3 a perfiles — necesita ADR propia.
4. Aprobar o rechazar ADR-0045.
5. Auditar una muestra estratificada de criterio humano: los pesos del
   representante vienen probablemente de la ejecución original conducida por un
   modelo y nadie los ha validado.

**Puerta de calidad** (pasarla entera antes de declarar nada cerrado):

```bash
cargo fmt --all --check
cargo clippy --workspace --exclude dataforge-desktop --all-targets -- -D warnings
DF_REQUIRE_HARDENING=1 cargo test --workspace --exclude dataforge-desktop
pnpm --filter dataforge-desktop format:check
pnpm --filter dataforge-desktop typecheck
pnpm --filter dataforge-desktop lint
pnpm --filter dataforge-desktop test:ui
pnpm --filter dataforge-desktop build
cargo check -p dataforge-desktop
```

`DF_REQUIRE_HARDENING=1` no es opcional: sin ella, un test de endurecimiento
que no pueda crear junctions imprime `SKIP` y pasa, y el verde no significa
nada.

## 4. Registro de sesiones

Append-only, lo más reciente arriba. Una línea por sesión: **dónde, qué, y qué
queda**. Añadir arriba y no reescribir lo anterior evita conflictos entre
equipos.

| Fecha | Dónde | Qué pasó | Queda |
| --- | --- | --- | --- |
| 2026-08-09 | PC | Auditoría del repo; ADRs de `perf/m101` renumerados a 0046/0047 con test de unicidad; clase `commit` cerrada en `df-mcp`; 1.0.1 preparada (#48); Dependabot triado (#49) con check de deriva de SBOM en CI | Mergear #48/#45/#49; retirar el borrador de la 1.0.0 |
| 2026-08-09 | PC | Ensayo completo del flujo CLI sobre corpus que replica el caso jurídico (4 puestos): `COMPLETED`, ledger verificado. **El 100 % acabó en `90_DataForge_Review`** y el árbol limpio quedó vacío — es la regla 9, no un fallo | Avisarlo antes de enseñar un resultado |
| 2026-08-01 | Oficina | RFC-0002 aprobada y fusionada; los cuatro crates de la 2.0 creados (`df-tools`, `df-mcp`, `df-rules`, `df-agent`); ROADMAP-2.0 con definición de hecho falsable | Cablear: nada de eso conduce el motor todavía |
| ~2026-08-04 | Oficina | **Trabajo real sobre el archivo del cliente**: escaneo y hash completos, ~250.000 archivos pendientes de revisión | **Sin registrar.** Las cifras que cita ROADMAP-2.0 salen de aquí; la evidencia cruda no está en el repo (ver §5) |
| 2026-07-27 | PC | Endurecimiento del asistente antes de la prueba de campo: reanudación, corte por disco lleno, destino degradado preguntado antes de copiar | — |

## 5. Huecos conocidos

Cosas que se hicieron y **no** están en el repositorio. Se listan para que nadie
las dé por perdidas ni las repita a ciegas.

- **Las pruebas de la 1.0 en la oficina.** De ahí salen las cifras que sostienen
  medio ROADMAP-2.0 — 158.219 archivos, 443,9 GB, 28.537 conjuntos de
  duplicados, 239,7 GB redundantes, 5.334 items de revisión de los que 3.702
  son una sola clase. Los números están citados; **la ejecución que los produjo
  no está registrada**. Mientras siga así, no son reproducibles por nadie más.
- **Los pesos del representante** proceden probablemente de la ejecución
  original conducida por un modelo, y no se han validado contra criterio humano.

Cerrar un hueco es escribir un fichero en
[`testing/field-runs/`](testing/field-runs/README.md), aunque sea de memoria y
diga que es de memoria.
