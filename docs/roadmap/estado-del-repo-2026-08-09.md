# Estado del repositorio — auditoría del 2026-08-09

Foto completa del proyecto para retomarlo desde cualquier equipo sin
reconstruir el contexto a base de leer ramas. Todo lo que aquí figura está
verificado contra el repositorio en la fecha indicada, no recordado.

**`main`:** `5bd73bd` · **Tag más reciente:** `v1.0.0` · **Release:** borrador,
sin publicar.

---

## 1. Qué es esto ahora mismo

Dos cosas a la vez, y conviene no confundirlas:

- **Un producto 1.0 terminado y sin publicar.** Motor de reconstrucción
  verificable: inventaría un origen sin tocarlo, prueba identidad con doble
  hash, detecta duplicados y árboles injertados, copia bajo manifiesto
  congelado y verifica de forma independiente. Probado hasta 1.000.000 de
  archivos.
- **Una fase 2.0 en curso** en una sola rama: que un agente conduzca ese motor
  sin sostener un shell, y que la salida tenga significado y no solo estructura.

## 2. Ramas vivas

| Rama | Ante `main` | PR | Estado |
| --- | --- | --- | --- |
| `feat/agent-drivable-engine` | +27 / -0 | [#45](https://github.com/luiscor97/dataforge/pull/45) | **La línea activa.** Toda la 2.0 |
| `perf/m101-pipeline-throughput` | +27 / -6 | [#30](https://github.com/luiscor97/dataforge/pull/30) borrador | Rendimiento. **Colisiona con `main`**, ver §5 |
| `fix/desktop-usability` | +4 / -2 | [#35](https://github.com/luiscor97/dataforge/pull/35) borrador | Selector de carpetas + instalador NSIS |
| `feat/guided-flow-hardening` | +8 / -4 | — | **Ya mergeada** vía #34; la rama sobra |
| `fix/source-root-read-only` | +1 / -4 | — | **Ya mergeada** vía #33; la rama sobra |
| `feat/guided-flow` | +1 / -5 | — | **Ya mergeada** vía #32; la rama sobra |

Además hay **nueve ramas de milestones cerrados** (`feat/m01-*`, `feat/m02-*`,
`feat/m09-*`, `hardening/*`, `release/v1.0.0`, `wip/*`, `docs/pre-release-*`)
que están entre 13 y 76 commits por detrás y no aportan nada: son historia ya
fusionada. Borrarlas reduce el ruido de `git branch -r`, que es justo lo que
provocó que una sesión empezara sin saber que las otras existían.

## 3. Estado de la 1.0

Cerrada y **sin publicar por decisión deliberada** (ADR-0039: publicar es un
acto humano).

- Tag `v1.0.0` empujado, workflow ejecutado, **release en borrador con 10
  artefactos** (4 binarios, `SHA256SUMS.txt`, SBOM, y sus firmas Sigstore).
- **`main` va 5 commits por delante del tag.** Los binarios del borrador son
  anteriores al asistente guiado (#32), al endurecimiento (#34) y al arreglo de
  la raíz de origen (#33). Publicar ese borrador tal cual entregaría software
  peor que el que hay en `main`. **Decisión pendiente:** o se retira el
  borrador y se taggea 1.0.1 desde `main`, o se publica sabiendo qué contiene.

## 4. Estado de la 2.0

El plan es RFC-0002 (aprobada, en `main`), el destino está en `ROADMAP-2.0.md`
y la secuencia en `continuidad-fase-agentica-2026-08-09.md`.

| Hito | Qué existe | Qué falta |
| --- | --- | --- |
| M2.1 Superficie agéntica | `df-tools` (25 herramientas, 3 clases), `df-mcp` (stdio, sin red), `Actor::Agent`, `decide-batch`, `plan tree` | Que `df-agent` conduzca por aquí |
| M2.2 Taxonomía de destino | `DestinationTaxonomy` por perfil, procedencia de enrutado (migración 0020) | `revisar/` como espejo del árbol |
| M2.3 Clasificación | Primera clasificación por `TREE_EMBEDDED` | **Bloqueado**: decidir el reencuadre a perfiles y resolver ADR-0045 |
| M2.4 `df-rules` | Núcleo puro, 4 fronteras duras, parámetros con digest | Persistencia (migración 0021) y **ser el gate de verdad** |
| M2.5 Consentimiento | Decisión pura con presupuesto | Persistencia y conexión al transporte |
| M2.6 `df-agent` | Fases, presupuestos, cortacircuitos | Que conduzca el motor |

**La regla que evita el error más probable:** un hito no está hecho porque sus
tipos existan. Los cuatro crates existen; lo que falta es cableado, persistencia
y prueba real.

## 5. Deuda concreta y accionable

### 5.1 Colisión de numeración de ADR — real, contra `main`

| ADR | `main` | `feat/agent-drivable` | `perf/m101` |
| --- | --- | --- | --- |
| 0040 | — | `declared-destination-taxonomy` | `bounded-parallel-hash-and-verify` |
| 0041 | `df-rules-canonical-recovery` | — | `strict-parallel-execution` |

`perf/m101` colisiona **dos veces**: su 0041 contra `main` (que ya ganó) y su
0040 contra la rama activa. Debe renumerar ambas a **0046 y 0047**. Los números
0004–0010 están libres pero **no deben reutilizarse**: el README de ADR los
tiene declarados como el rango de las decisiones fundacionales.

Nada detecta esto automáticamente. Falta un test de unicidad.

### 5.2 Dependabot: 15 PRs abiertas

Ninguna se ha mirado. Dos avisos que valen para todas:

- Su base de CI está desactualizada; hay que pedir `@dependabot rebase` antes de
  juzgar sus checks.
- **Mergear cualquiera de las de Rust obliga a regenerar
  `docs/sbom/dataforge.cdx.json` en el mismo cambio**, o el job `SBOM
  (regenerate and verify)` falla en el siguiente tag y no sale release. Ya pasó
  una vez con `tauri-plugin-dialog`.

Saltos mayores que necesitan mirada: `jsonschema` 0.33→0.49, `vite` 7→8,
`jsdom` 27→30, y los `actions/*-artifact` 4→7/8.

### 5.3 Decisiones que bloquean el siguiente bloque grande

1. **Reencuadre de M2.3 a perfiles.** Necesita ADR antes de implementar más
   clasificación. Hasta cerrarla no conviene persistir parámetros cuya semántica
   pueda cambiar.
2. **ADR-0045** está en Propuesta con una recomendación explícita, pendiente de
   tu aprobación. Desbloquea 118,9 GB de los 234,2 GB de redundancia bloqueada.
3. **Auditar una muestra de criterio humano.** Los pesos del representante
   proceden probablemente de la ejecución original conducida por un modelo y
   nadie los ha validado.

## 6. Contratos y esquema

| | `main` | `feat/agent-drivable` |
| --- | --- | --- |
| Migraciones | 19 | 20 (`routing_provenance`) |
| `frozen_contracts` espera | 19 | 20 |
| `PROFILE_SCHEMA_VERSION` | 1.1.0 | 2.0.0 |

La disciplina de ADR-0037 §2 se está respetando: cada contrato que se mueve
actualiza su expectativa **en el mismo commit**. Un test de contratos roto sin
ADR es un accidente que revertir, nunca un test que actualizar.

## 7. Puerta de calidad

Lo que hay que pasar antes de declarar nada cerrado:

```bash
cargo fmt --all --check
cargo clippy --workspace --exclude dataforge-desktop --all-targets -- -D warnings
cargo test --workspace --exclude dataforge-desktop
pnpm --filter dataforge-desktop format:check
pnpm --filter dataforge-desktop typecheck
pnpm --filter dataforge-desktop lint
pnpm --filter dataforge-desktop test:ui
pnpm --filter dataforge-desktop build
cargo check -p dataforge-desktop
```

**Verificado en `feat/agent-drivable-engine` el 2026-08-09: 532 tests Rust, 0
fallos; escritorio 41, 0 fallos.**

Corre los tests con **`DF_REQUIRE_HARDENING=1`**: sin esa variable, un test de
endurecimiento que no pueda crear junctions imprime `SKIP` y pasa, y el verde no
significa nada. CI la fija en el job de Windows.

### CI

Seis jobs: `rust`, `desktop`, `hardening` (Windows), `supply-chain`,
`rust-linux-experimental` y `fuzz`. Los dos últimos son experimentales pero
**han encontrado fallos reales** — el de Linux detectó código muerto que la
puerta de Windows no podía ver.

`release.yml` **solo corre en tag o `workflow_dispatch`**. No se ejercita en
PRs, así que un fallo de SBOM o de firma no aparece hasta que taggeas.

## 8. Entorno para reproducir

| | Requerido | En este equipo |
| --- | --- | --- |
| Rust | `rust-version = 1.88` | 1.97.0 |
| Node | 24 (CI) | 26.3.0 |
| pnpm | v4 action | 10.34.5 |

**Windows con toolchain GNU** (ADR-0011): el `PATH` necesita `~/.cargo/bin` y
el `mingw64/bin` de WinLibs. Hay toolchain MSVC instalado también; el escritorio
compila con GNU pese al aviso de `.rsrc merge failure`, que es inocuo.

`pnpm` solo funciona desde PowerShell en este equipo, no desde el shell POSIX.

La ejecución de copias **falla cerrado fuera de Windows** a propósito; el resto
del análisis funciona en Linux.

## 9. Riesgos de coordinación

Esto ya ha mordido dos veces: `Actor::Agent` se implementó dos veces, y ADR-0040
respondió una pregunta que RFC-0002 ya había contestado mejor.

Antes de abrir rama: `git branch -r`, y leer
`docs/roadmap/estado-superficie-agentica.md`.

**Carriles que no se mezclan:** `fix/desktop-usability` es producto e
instalador; `perf/m101` es rendimiento y no debe tocar el executor antes de
estabilizar recuperación y procedencia; la UI final se diseña sobre un L1
funcional.

## 10. Si mañana solo puedes hacer una cosa

En este orden, y cada una cabe en una sesión:

1. **Cerrar la #45.** Es la línea activa, pasa la puerta, y cuanto más crezca
   más difícil es de revisar. El PR 0 del traspaso.
2. **Renumerar `perf/m101` a 0046/0047** y añadir el test de unicidad de ADR.
3. **Decidir qué hacer con el borrador de release**, que hoy contiene software
   peor que `main`.
4. **Podar las ramas ya fusionadas.**
5. Dependabot, con la advertencia del SBOM por delante.
