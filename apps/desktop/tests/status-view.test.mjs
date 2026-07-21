import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const appRoot = fileURLToPath(new URL("../", import.meta.url));

let vite;
let StatusView;

before(async () => {
  vite = await createServer({
    root: appRoot,
    appType: "custom",
    logLevel: "silent",
    server: { middlewareMode: true },
  });
  ({ StatusView } = await vite.ssrLoadModule("/src/screens/StatusView.tsx"));
});

after(async () => {
  await vite?.close();
});

const baseStatus = {
  project_id: "project-1",
  name: "Proyecto de prueba",
  state: "READY",
  profile: "balanced",
  app_version: "0.2.0",
  created_at: "2026-07-17T09:00:00Z",
  updated_at: "2026-07-17T09:05:00Z",
  project_dir: "C:\\dataforge\\project-1",
  output_root: "C:\\dataforge\\project-1\\output",
  audit_root: "C:\\dataforge\\project-1\\audit",
  source_roots: [],
  event_count: 0,
  last_event: null,
  latest_snapshot_id: null,
  inventory: null,
  structural_diagnostics: null,
  similarity: null,
  media: null,
  integrity: null,
};

function renderStatus(status) {
  return renderToStaticMarkup(
    createElement(StatusView, {
      status,
      busy: false,
      onRefresh() {},
      onBack() {},
    }),
  );
}

test("announces the structural diagnosis as pending before the first snapshot", () => {
  const markup = renderStatus(baseStatus);

  assert.match(
    markup,
    /<section class="diagnostics" aria-labelledby="structural-diagnostics-heading">/,
  );
  assert.match(markup, /<p class="diagnostic-status diagnostic-pending" role="status">/);
  assert.match(markup, /Pendiente: primero hay que completar el escaneo/);
  assert.doesNotMatch(markup, /class="diagnostics-grid"/);
});

test("does not present partial counters as final results", () => {
  const markup = renderStatus({
    ...baseStatus,
    latest_snapshot_id: "snapshot-1",
    structural_diagnostics: {
      analysis_complete: false,
      folder_signatures: 99,
      exact_tree_clone_sets: 98,
      partial_tree_clones: 97,
      embedded_trees: 96,
      repeated_components: 95,
      candidate_cap_reached: true,
      generic_folders: 94,
      protected_boundaries: 93,
      rule_matches: 92,
      anomalies: 91,
      high_anomalies: 90,
      pending_review: 89,
    },
  });

  assert.match(markup, /role="status">Análisis estructural pendiente o incompleto/);
  assert.doesNotMatch(markup, /class="diagnostics-grid"/);
  assert.doesNotMatch(markup, /Componentes repetidos/);
});

test("announces completion and renders the REPEATED_COMPONENT_ONLY counter", () => {
  const markup = renderStatus({
    ...baseStatus,
    latest_snapshot_id: "snapshot-1",
    structural_diagnostics: {
      analysis_complete: true,
      folder_signatures: 3,
      exact_tree_clone_sets: 4,
      partial_tree_clones: 5,
      embedded_trees: 6,
      repeated_components: 12345,
      candidate_cap_reached: true,
      generic_folders: 7,
      protected_boundaries: 8,
      rule_matches: 9,
      anomalies: 10,
      high_anomalies: 11,
      pending_review: 12,
    },
  });

  assert.match(
    markup,
    /<span class="diagnostic-badge diagnostic-complete" role="status">Análisis completo<\/span>/,
  );
  assert.match(
    markup,
    /<section class="diagnostic-group" aria-labelledby="diagnostic-trees-heading">/,
  );
  assert.match(markup, /<h4 id="diagnostic-trees-heading">Estructura de árboles<\/h4>/);
  assert.match(markup, /<dt>Componentes repetidos<\/dt><dd>12\.345<\/dd>/);
  assert.match(markup, /Límite de candidatos/);
  assert.match(markup, /Alcanzado — resultados no exhaustivos/);
});

test("renders sealed M0.3 version evidence without implying an automatic action", () => {
  const markup = renderStatus({
    ...baseStatus,
    latest_snapshot_id: "snapshot-1",
    structural_diagnostics: {
      analysis_complete: true,
      folder_signatures: 1,
      exact_tree_clone_sets: 0,
      partial_tree_clones: 0,
      embedded_trees: 0,
      repeated_components: 0,
      candidate_cap_reached: false,
      generic_folders: 0,
      protected_boundaries: 0,
      rule_matches: 0,
      anomalies: 0,
      high_anomalies: 0,
      pending_review: 0,
    },
    similarity: {
      run_id: "run-1",
      snapshot_id: "snapshot-1",
      algorithm_version: "fastcdc-test",
      config_digest: "a".repeat(64),
      config: {
        algorithm_version: "fastcdc-test",
        options: {
          min_chunk_bytes: 16384,
          avg_chunk_bytes: 65536,
          max_chunk_bytes: 262144,
          min_file_bytes: 16384,
          threshold: 0.5,
          min_shared_chunks: 2,
          min_shared_bytes: 32768,
          minhash_permutations: 128,
          lsh_bands: 32,
          max_bucket_contents: 64,
          max_candidates: 200000,
        },
      },
      counters: {
        contents_total: 3,
        contents_chunked: 3,
        contents_skipped: 0,
        chunks_total: 20,
        candidates_total: 1,
        relations_total: 1,
      },
      candidate_cap_reached: false,
      relationships_truncated: false,
      relationships: [
        {
          id: "relation-1",
          content_a: "a",
          content_b: "b",
          path_a: "C:\\docs\\contrato-v1.pdf",
          path_b: "C:\\docs\\contrato-v2.pdf",
          kind: "LIKELY_VERSION",
          direction: "A_TO_B",
          similarity: 0.875,
          shared_chunks: 7,
          shared_bytes: 123456,
          union_bytes: 140000,
          estimated_similarity: 0.86,
          confidence: 0.9,
          evidence: {},
        },
      ],
    },
  });

  assert.match(markup, /Similitud y versiones/);
  assert.match(markup, /role="status">Evidencia sellada/);
  assert.match(markup, /LIKELY_VERSION/);
  assert.match(markup, /87\.5%/);
  assert.match(markup, /<dt>Umbral exacto<\/dt><dd>50\.0%<\/dd>/);
  assert.match(markup, /<dt>Máximo de candidatos<\/dt><dd>200\.000<\/dd>/);
  assert.match(markup, /la similitud no equivale a identidad/);
  assert.match(markup, /nunca autoriza por sí sola/);
});

test("renders sealed M0.5 media evidence without implying an automatic action", () => {
  const markup = renderStatus({
    ...baseStatus,
    latest_snapshot_id: "snapshot-1",
    media: {
      run_id: "media-run-1",
      snapshot_id: "snapshot-1",
      contract_version: "dataforge.media-analysis.v1",
      config_digest: "b".repeat(64),
      config: {},
      counters: {
        contents_total: 3,
        contents_analyzed: 3,
        contents_limited: 0,
        contents_failed: 0,
        pairs_compared: 3,
        relations_total: 1,
      },
      pair_cap_reached: false,
      relations_truncated: false,
      relations: [
        {
          relation: "IMAGE_PERCEPTUAL_MATCH",
          score_millionths: 968750,
          content_a: "a",
          content_b: "b",
          path_a: "fotos\\viaje.jpg",
          path_b: "fotos\\viaje-comprimida.jpg",
          evidence: { kind: "IMAGE", hamming_distance: 2 },
        },
      ],
    },
  });

  assert.match(markup, /Inteligencia multimedia/);
  assert.match(markup, /role="status">Evidencia sellada/);
  assert.match(markup, /IMAGE_PERCEPTUAL_MATCH/);
  assert.match(markup, /96\.9%/);
  assert.match(markup, /viaje-comprimida\.jpg/);
  assert.match(markup, /<dt>Pares comparados<\/dt><dd>3<\/dd>/);
  assert.match(markup, /una coincidencia perceptual/);
  assert.match(markup, /nunca autoriza por sí sola/);
});

test("keeps the media diagnosis pending until structural analysis completes", () => {
  const markup = renderStatus({
    ...baseStatus,
    latest_snapshot_id: "snapshot-1",
  });

  assert.match(markup, /Inteligencia multimedia/);
  assert.match(
    markup,
    /Pendiente: primero debe terminar el análisis estructural/,
  );
});
