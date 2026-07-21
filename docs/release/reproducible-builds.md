# Builds reproducibles

Objetivo M0.9: que el binario que se distribuye sea función determinista
del código fuente y del `Cargo.lock`, no del momento o de la máquina que
lo compiló. Esto es lo que hace verificable una release: cualquiera puede
recompilar y comparar hashes.

## Qué está fijado por diseño

- **Dependencias ancladas**: `Cargo.lock` versionado; los builds de release
  usan `--locked`, así que un grafo de dependencias distinto no compila en
  silencio — falla.
- **Sin timestamps embebidos en el código**: ni el código ni `build.rs`
  propios embeben hora de build; el SBOM tampoco (es byte-idéntico por
  diseño, `docs/sbom/README.md`).
- **Linkado sin reloj (ADR-0038)**: la cabecera PE lleva un `TimeDateStamp`
  que el linker rellena con la hora real por defecto — eso hace único cada
  build aunque todo lo demás sea determinista. `.cargo/config.toml` pasa
  `/Brepro` al linker MSVC (la toolchain de CI/release), que lo sustituye
  por un hash reproducible del contenido.
- **Toolchain declarada**: la CI y el workflow de release instalan la misma
  toolchain estable (`dtolnay/rust-toolchain@stable`).

## El hallazgo (2026-07-21, máquina de desarrollo GNU)

El doble build limpio inicial de los cuatro binarios (`dataforge`,
`df-extract-worker`, `df-query-worker`, `df-media-worker`) **no fue
byte-idéntico**. Causa raíz verificada leyendo la cabecera COFF: el
`TimeDateStamp` PE contenía la hora real de linkado (p. ej. `1784596624 →
2026-07-21 01:17:04`). No era un problema del código: era el linker.

Con `-Wl,--no-insert-timestamp` (GNU ld), un rebuild completo desde cero
produjo los cuatro binarios con hash estable en el pass que completó:

```text
e3b338a88768fb37474774cf707a01777ee0c746269714ec3a2f42112dc4c2ea  dataforge.exe
148d9889d7acb1a43c1304d1240323a706c46462a680a3d73945d86bc78f4a7d  df-extract-worker.exe
73611429917836771df53a27c79f33a9fdf48905bac283ce6fd932936d865aca  df-query-worker.exe
2cc7cc249bb6c18e4556253775c7c837767e412213ad9ae5a275c4816c3825f5  df-media-worker.exe
```

La confirmación local A/B quedó **bloqueada por Windows Defender**, que
puso en cuarentena build scripts recién linkados en mitad del segundo pass
(os error 225, detecciones registradas sobre
`target/release/build/*/build_script_build*.exe`): los PE con timestamp a
cero disparan su heurística. Ese es el motivo de que el flag **no** se
aplique a la toolchain GNU de desarrollo — pelear con el antivirus en cada
build local no es un coste aceptable, y silenciar el AV no es decisión
nuestra sino del propietario de la máquina.

## Verificación canónica

La prueba oficial es el job **`Reproducibility (double clean rebuild)`**
de `.github/workflows/release.yml`: dos builds completos desde cero en un
runner limpio (MSVC + `/Brepro`, sin caché), comparados por SHA-256, que
**fallan la release si difieren**. Se puede ensayar sin tag con
`workflow_dispatch`.

## Límites declarados

- La verificación prueba **rebuild determinista en el mismo entorno, misma
  ruta y misma toolchain** — la forma en que un usuario verifica "¿este
  binario sale de este código?" recompilando con la misma toolchain.
- **No** prueba independencia de la ruta del workspace ni identidad
  bit-a-bit entre máquinas u OS distintos: rustc embebe rutas absolutas en
  paths de pánico y debug info. Cerrar eso requiere `--remap-path-prefix`
  y un entorno de build canónico (contenedor); queda como refuerzo
  post-1.0, documentado aquí para que la promesa nunca sea mayor que la
  prueba.
- En máquinas de desarrollo con antivirus activo, la verificación local
  puede requerir una exclusión para `target/` — decisión del propietario
  de la máquina, nunca del build.
