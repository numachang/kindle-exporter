"""CUDA EP の失敗原因を特定し、CPU 側の現実的なスループットを測る。"""

import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import onnxruntime as ort

ort.set_default_logger_severity(3)

BASE = Path(__file__).resolve().parent / "ndlocr-lite" / "src"
PARSEQ = BASE / "model" / "parseq-ndl-24x768-100-tiny-153epoch-tegaki3-r8data-202604.onnx"
DEIM = BASE / "model" / "deim-s-1024x1024.onnx"

print("=" * 70)
print("1. PARSeq を CUDA で走らせたときの完全なエラー")
print("=" * 70)
try:
    s = ort.InferenceSession(str(PARSEQ), providers=["CUDAExecutionProvider", "CPUExecutionProvider"])
    print(f"   providers: {s.get_providers()}")
    x = np.random.rand(1, 3, 24, 768).astype(np.float32)
    s.run(None, {"images": x})
    print("   → 成功")
except Exception as e:
    print(f"   {type(e).__name__}: {e}")

print("\n" + "=" * 70)
print("2. CUDA EP に落ちなかったノードを確認（fallback 状況）")
print("=" * 70)
opts = ort.SessionOptions()
opts.log_severity_level = 1
opts.optimized_model_filepath = ""
try:
    s2 = ort.InferenceSession(
        str(PARSEQ),
        providers=[("CUDAExecutionProvider", {"device_id": 0}), "CPUExecutionProvider"],
    )
    prof = s2.get_provider_options()
    print(f"   provider options: {list(prof.keys())}")
except Exception as e:
    print(f"   {type(e).__name__}: {str(e)[:200]}")

print("\n" + "=" * 70)
print("3. DEIM（レイアウト検出）— 入力2つを正しく渡して計測")
print("=" * 70)
for prov in ["CPUExecutionProvider", "CUDAExecutionProvider"]:
    try:
        s3 = ort.InferenceSession(str(DEIM), providers=[prov, "CPUExecutionProvider"])
        if prov not in s3.get_providers():
            print(f"   [{prov}] 載らず → skip")
            continue
        for bs in ([1] if prov == "CPUExecutionProvider" else [1, 4, 8]):
            feed = {
                "images": np.random.rand(bs, 3, 800, 800).astype(np.float32),
                "orig_target_sizes": np.tile(np.array([[800, 800]], dtype=np.int64), (bs, 1)),
            }
            s3.run(None, feed)
            t0 = time.perf_counter()
            for _ in range(6):
                s3.run(None, feed)
            dt = (time.perf_counter() - t0) / 6
            print(f"   [{prov[:4]}] batch={bs}: {dt * 1000:7.1f} ms/回  {dt / bs * 1000:6.1f} ms/ページ")
    except Exception as e:
        print(f"   [{prov}] {type(e).__name__}: {str(e)[:150]}")

print("\n" + "=" * 70)
print("4. PARSeq CPU の現実的スループット（ocr.py と同じ 1スレッド/セッション × 並列）")
print("=" * 70)


def make_cpu_session():
    o = ort.SessionOptions()
    o.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    o.intra_op_num_threads = 1
    o.inter_op_num_threads = 1
    return ort.InferenceSession(str(PARSEQ), o, providers=["CPUExecutionProvider"])


x = np.random.rand(1, 3, 24, 768).astype(np.float32)
for workers in [1, 4, 8, 16]:
    sessions = [make_cpu_session() for _ in range(workers)]
    for s in sessions:
        s.run(None, {"images": x})
    n_lines = 64

    def work(i):
        sessions[i % workers].run(None, {"images": x})

    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as ex:
        list(ex.map(work, range(n_lines)))
    dt = time.perf_counter() - t0
    print(f"   {workers:2d} 並列: {n_lines} 行を {dt * 1000:7.1f} ms → {dt / n_lines * 1000:6.2f} ms/行  "
          f"({n_lines / dt:6.1f} 行/秒)")

print("\n（実書籍 1 ページ ≒ 20 行と仮定した 1 ページあたり所要時間は上の ms/行 × 20 + 検出時間）")
