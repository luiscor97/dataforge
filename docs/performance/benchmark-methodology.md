# Metodología de benchmarks (M1.0.1)

Cómo se mide DataForge para que cada cifra publicada sea reproducible y
ninguna mejora se afirme sin comparación válida.

## Corpus

Cuatro perfiles deterministas generados por `df-corpus --profile` (misma
semilla + perfil + recuento = árbol byte-idéntico en cualquier máquina):

| Perfil | Forma | Recuento por defecto |
| --- | --- | --- |
| `a-small` | 100 % entre 1 B y 16 KiB | 100.000 |
| `b-mixed` | 70 % <64 KiB · 20 % 64 KiB–10 MiB · 9 % 10–64 MiB · 1 % 64–256 MiB | 10.000 |
| `c-large` | 100–2048 MiB por archivo | 50 |
| `d-million` | forma legacy M0.8 (pequeños + ~1 MiB cada 500) | 1.000.000 |

Los tamaños dentro de cada banda se muestrean **log-uniformemente con
aritmética entera** (bucket potencia-de-dos + resto uniforme): los tamaños
pequeños dominan como en colecciones reales y ninguna diferencia de
redondeo flotante puede hacer divergir dos plataformas.

Desviación declarada del encargo: el perfil B literal (100.000 archivos
con 9 % de 10–250 MiB) ocupa >1 TB; el recuento por defecto baja a 10.000
con la misma forma, y el literal queda opt-in vía `--files 100000` para
quien tenga el espacio. La banda alta se acota a 256 MiB por la misma
razón.

## Driver

`scripts/bench/run-pipeline-bench.ps1` ejecuta el pipeline completo
(create → scan → hash → analyze → plan → approve → execute → verify) con
el CLI **release `--locked`**, fase a fase, y registra por fase:

- segundos de pared (`Stopwatch`);
- working set pico y tiempo de CPU muestreados cada 250 ms;
- código de salida y stdout `--json` completo (queda en el caso);
- throughput derivado: archivos/s (scan, hash, execute, verify) y MiB/s
  (hash, execute, verify).

Cada resultado se escribe en `docs/performance/data/<caso>.json` con
commit, build, hardware, SO, filesystem, perfil, recuento, semilla y hora
UTC. El nombre del caso incluye todo lo necesario para regenerarlo.

## Reglas

1. **Baseline primero**: ninguna optimización se afirma sin un caso
   `baseline` previo en el mismo hardware, perfil, recuento y semilla.
2. **Mismo corpus, misma semilla**: las comparativas solo son válidas
   entre casos con (perfil, files, seed) idénticos.
3. **Release siempre**: nunca se publica una cifra de una build debug.
4. **Sin telemetría**: toda la instrumentación es local y opcional.
5. **Pruebas no realizadas se marcan**, no se estiman: si una fase falla o
   se interrumpe, su fila queda con el código de salida real.
6. Los resultados de máquinas con antivirus activo pueden estar sesgados
   por escaneos on-access (visto en M0.9: Defender puso en cuarentena
   binarios recién linkados); anotar el estado del AV cuando se conozca.

## Reproducir

```powershell
# baseline del perfil A en la máquina actual
powershell -File scripts/bench/run-pipeline-bench.ps1 -Profile a-small -Label baseline

# comparativa tras un cambio, mismo corpus
powershell -File scripts/bench/run-pipeline-bench.ps1 -Profile a-small -Label buffers-256k
```

Los corpus y salidas viven fuera del repo (`-Root`, por defecto
`%USERPROFILE%\Desktop\dataforge-bench`) y se limpian al terminar salvo
`-KeepCorpus`.
