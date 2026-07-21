# Builds reproducibles

Objetivo M0.9: que el binario que se distribuye sea función determinista
del código fuente y del `Cargo.lock`, no del momento o de la máquina que
lo compiló. Esto es lo que hace verificable una release: cualquiera puede
recompilar y comparar hashes.

## Qué está fijado por diseño

- **Dependencias ancladas**: `Cargo.lock` versionado; los builds de release
  usan `--locked`, así que un grafo de dependencias distinto no compila en
  silencio — falla.
- **Sin timestamps embebidos**: ni el código ni `build.rs` propios embeben
  hora de build; el SBOM tampoco (es byte-idéntico por diseño,
  `docs/sbom/README.md`).
- **Toolchain declarada**: la CI y el workflow de release instalan la misma
  toolchain estable (`dtolnay/rust-toolchain@stable`); la identidad exacta
  de `rustc` queda registrada en la evidencia.

## Evidencia local (rebuild determinista)

Método: compilar los cuatro binarios de release (`dataforge`,
`df-extract-worker`, `df-query-worker`, `df-media-worker`) con
`cargo build --release --locked`, tomar SHA-256, **borrar por completo
`target/release`**, recompilar desde cero y comparar.

<!-- EVIDENCE -->

## Límites declarados

- La evidencia prueba **rebuild determinista en la misma máquina, misma
  ruta y misma toolchain** — la forma que un usuario usa para verificar
  "¿este binario sale de este código?" recompilando en su entorno con la
  misma toolchain.
- **No** prueba independencia de la ruta del workspace ni identidad
  bit-a-bit entre máquinas u OS distintos: rustc embebe rutas absolutas en
  paths de pánico y debug info, así que compilar desde otro directorio
  puede producir bytes distintos. Cerrar eso requiere `--remap-path-prefix`
  y un entorno de build canónico (contenedor); queda como refuerzo
  post-1.0, documentado aquí para que la promesa nunca sea mayor que la
  prueba.
- La verificación cruzada oficial es el job de release: misma toolchain
  estable, mismo `--locked`, checksums publicados junto a los binarios.
