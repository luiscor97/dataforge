# Política de seguridad

## Versiones soportadas

DataForge está en fase de fundación (`0.0.x-dev`). No hay releases con
soporte todavía; los informes de seguridad se aplican a `main`.

## Cómo reportar una vulnerabilidad

**No abras un issue público.**

Envía un correo a **luiscor97@gmail.com** con asunto `[SECURITY] DataForge`
incluyendo:

- descripción y impacto;
- pasos de reproducción o PoC;
- versión/commit afectado;
- si procede, propuesta de mitigación.

Compromisos:

- acuse de recibo en 7 días;
- evaluación inicial y plan en 30 días;
- crédito en las notas de la corrección si lo deseas.

Cuando el repositorio esté publicado en GitHub se habilitará además
*GitHub Private Vulnerability Reporting* como canal preferente.

## Ámbito

Nos interesa especialmente cualquier vía por la que el motor pudiera:

1. modificar, borrar o sobrescribir archivos de origen;
2. escribir fuera de la raíz de salida (path traversal, symlink escape);
3. falsificar o romper el ledger de auditoría sin detección;
4. ejecutar código a través de documentos o rutas hostiles.
