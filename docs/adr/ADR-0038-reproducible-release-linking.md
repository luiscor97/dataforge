# ADR-0038 — Linkado reproducible de release (M0.9)

- Estado: Aceptada
- Fecha: 2026-07-21
- Contexto: RFC-0001 §M0.9 (builds reproducibles);
  `docs/release/reproducible-builds.md`

## Contexto

Un doble build limpio de los binarios de release no era byte-idéntico. La
causa raíz, verificada leyendo la cabecera COFF, es el `TimeDateStamp` PE:
tanto GNU `ld` como MSVC `link.exe` lo rellenan con la hora real de
linkado por defecto. Sin arreglarlo, "builds reproducibles" sería una
promesa vacía: cada build es único aunque código, dependencias y toolchain
sean idénticos.

Durante la validación local apareció una restricción operativa real: los
PE con timestamp a cero disparan heurísticas de Windows Defender, que
puso en cuarentena build scripts recién linkados en mitad de un build
(os error 225). Un flag global castigaría cada build de desarrollo en
Windows.

## Decisión

1. `.cargo/config.toml` pasa `/Brepro` al linker **solo para
   `x86_64-pc-windows-msvc`** — la toolchain de CI y release. `/Brepro`
   sustituye el timestamp por un hash reproducible del contenido.
2. La toolchain GNU de desarrollo (ADR-0011) conserva el linkado por
   defecto. La verificación local con `-Wl,--no-insert-timestamp` queda
   documentada como opcional, con su interacción con antivirus declarada.
3. La verificación canónica es el job `Reproducibility (double clean
   rebuild)` del workflow de release: dos builds completos desde cero en
   runner limpio, comparados por SHA-256, que **bloquean la release** si
   difieren. Ensayable sin tag vía `workflow_dispatch`.

## Consecuencias

- La reproducibilidad se prueba donde se produce el artefacto que se
  distribuye, no donde es hostil probarla; el entorno de desarrollo no
  paga el coste.
- La promesa queda acotada: rebuild determinista en el mismo entorno.
  Independencia de ruta/máquina (`--remap-path-prefix`, entorno canónico)
  es refuerzo post-1.0.
- Silenciar o excluir el antivirus de una máquina de desarrollo es
  decisión de su propietario; ninguna parte del build lo requiere ni lo
  hace.
