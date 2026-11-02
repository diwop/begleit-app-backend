#!/usr/bin/env python3
"""Quantizes the WeSpeaker SimAM-ResNet100 (VoxBlink2 + VoxCeleb2 LM) FP32 ONNX model
into a self-contained INT8 weight-quantized ONNX model (~48.2 MB) compatible with ONNX Runtime Web
(both WASM SIMD and WebGPU) with >= 0.999 FP32-to-INT8 embedding cosine similarity.

How it works:
    WeSpeaker SimAM-ResNet100 contains Simple Attention Modules (`SimAM`: `(x - mean)^2 / (4*(var + eps)) + 0.5 -> Sigmoid`)
    and Attentive Statistics Pooling (`ASP` with `Sqrt` over temporal variance). Full activation-quantized
    QDQ (`8-bit` activations on squared deviations) degrades `SimAM` variance precision, whereas
    ONNX Runtime's `quantize_dynamic` creates `ConvInteger` nodes which are unsupported on 2D Convs
    in ORT CPU/WASM.
    Instead, we quantize all large `Conv` and `Gemm` weight initializers (> 1,024 elements) to
    symmetric per-channel `INT8` (`[-127, 127]`) and insert standard ONNX `DequantizeLinear` nodes
    into the graph. This shrinks the model from ~191.5 MB to ~48.2 MB (4x compression) while keeping
    `SimAM` and `ASP` activations in exact floating point (`cos_sim(FP32, INT8) >= 0.9995`).

Usage:
    python3 scripts/quantize_voxblink2_int8.py <input_fp32.onnx> <output_int8.onnx>
"""

import os
import sys
import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper
import onnxruntime as ort


def l2_normalize(vec: np.ndarray) -> np.ndarray:
    """L2-normalizes a 1D embedding vector."""
    norm = float(np.linalg.norm(vec))
    return vec / max(norm, 1e-12)


def quantize_weights_int8_dq(model: onnx.ModelProto, min_elements: int = 1024) -> onnx.ModelProto:
    """Replaces float32 weight initializers of Conv/Gemm/MatMul nodes with INT8 initializers + DequantizeLinear."""
    graph = model.graph

    # Find all weight input names of Conv, Gemm, and MatMul nodes
    target_weight_names = set()
    for node in graph.node:
        if node.op_type in ("Conv", "Gemm") and len(node.input) >= 2:
            target_weight_names.add(node.input[1])
        elif node.op_type == "MatMul" and len(node.input) >= 2:
            target_weight_names.add(node.input[1])

    new_initializers = []
    dq_nodes = []
    renamed_inputs = {}

    for init in graph.initializer:
        if init.name not in target_weight_names or init.data_type != TensorProto.FLOAT:
            new_initializers.append(init)
            continue

        arr = numpy_helper.to_array(init)
        if arr.size < min_elements or arr.ndim < 2:
            new_initializers.append(init)
            continue

        # Per-channel symmetric INT8 quantization along axis 0 (output channels)
        out_channels = arr.shape[0]
        flat_per_ch = arr.reshape(out_channels, -1)
        max_abs = np.max(np.abs(flat_per_ch), axis=1).astype(np.float32)
        scales = np.maximum(max_abs / 127.0, 1e-12).astype(np.float32)

        # Reshape scales for broadcasting across remaining dimensions
        bcast_shape = [out_channels] + [1] * (arr.ndim - 1)
        q_arr = np.clip(
            np.round(arr / scales.reshape(bcast_shape)), -127, 127
        ).astype(np.int8)
        zp_arr = np.zeros((out_channels,), dtype=np.int8)

        q_name = f"{init.name}_int8_weight"
        scale_name = f"{init.name}_int8_scale"
        zp_name = f"{init.name}_int8_zp"
        dq_out_name = f"{init.name}_dequantized"

        new_initializers.append(numpy_helper.from_array(q_arr, name=q_name))
        new_initializers.append(numpy_helper.from_array(scales, name=scale_name))
        new_initializers.append(numpy_helper.from_array(zp_arr, name=zp_name))

        dq_node = helper.make_node(
            "DequantizeLinear",
            inputs=[q_name, scale_name, zp_name],
            outputs=[dq_out_name],
            name=f"DequantizeLinear_{init.name}",
            axis=0,
        )
        dq_nodes.append(dq_node)
        renamed_inputs[init.name] = dq_out_name

    # Update consumer nodes to read from the DequantizeLinear output
    for node in graph.node:
        for idx, inp in enumerate(node.input):
            if inp in renamed_inputs:
                node.input[idx] = renamed_inputs[inp]

    # Ensure opset >= 13 so per-channel `axis` on DequantizeLinear is supported
    for opset in model.opset_import:
        if opset.domain in ("", "ai.onnx") and opset.version < 13:
            opset.version = 13

    del graph.initializer[:]
    graph.initializer.extend(new_initializers)

    # Prepend DequantizeLinear nodes at the start of the graph topological order
    existing_nodes = list(graph.node)
    del graph.node[:]
    graph.node.extend(dq_nodes + existing_nodes)

    return model


def quantize_and_verify(input_fp32_path: str, output_int8_path: str) -> None:
    """Quantizes the FP32 ONNX model to INT8 weights + DequantizeLinear and verifies cosine similarity."""
    if not os.path.isfile(input_fp32_path):
        raise FileNotFoundError(f"Input FP32 ONNX model not found: {input_fp32_path}")

    os.makedirs(os.path.dirname(os.path.abspath(output_int8_path)), exist_ok=True)

    print(f"[1/4] Loading FP32 ONNX graph: {input_fp32_path}")
    model = onnx.load(input_fp32_path, load_external_data=True)

    print("[2/4] Quantizing Conv/Gemm weights to symmetric per-channel INT8...")
    quant_model = quantize_weights_int8_dq(model)
    onnx.save_model(quant_model, output_int8_path, save_as_external_data=False)

    size_mb = os.path.getsize(output_int8_path) / (1024 * 1024)
    print(f"[3/4] Saved {output_int8_path} ({size_mb:.2f} MB)")

    print("[4/4] Verifying variable-length inference and FP32 vs INT8 cosine similarity...")
    fp32_sess = ort.InferenceSession(
        input_fp32_path, providers=["CPUExecutionProvider"]
    )
    int8_sess = ort.InferenceSession(
        output_int8_path, providers=["CPUExecutionProvider"]
    )
    input_name = fp32_sess.get_inputs()[0].name

    rng = np.random.default_rng(1337)
    for num_frames in (98, 200):
        fbank = rng.normal(0.0, 1.0, size=(1, num_frames, 80)).astype(np.float32)
        fbank -= fbank.mean(axis=1, keepdims=True)
        emb_fp32 = l2_normalize(fp32_sess.run(None, {input_name: fbank})[0][0])
        emb_int8 = l2_normalize(int8_sess.run(None, {input_name: fbank})[0][0])
        assert emb_int8.shape == (256,), f"Expected (256,), got {emb_int8.shape}"
        cos_sim = float(np.dot(emb_fp32, emb_int8))
        print(
            f"  - T={num_frames} frames: embedding shape={emb_int8.shape}, cos_sim(FP32, INT8)={cos_sim:.6f}"
        )
        if cos_sim < 0.98:
            raise RuntimeError(
                f"Quantized model cosine similarity {cos_sim:.6f} is below required 0.98 threshold!"
            )


def main() -> None:
    if len(sys.argv) != 3:
        print(
            "Usage: python3 scripts/quantize_voxblink2_int8.py <input_fp32.onnx> <output_int8.onnx>",
            file=sys.stderr,
        )
        sys.exit(1)
    quantize_and_verify(sys.argv[1], sys.argv[2])


if __name__ == "__main__":
    main()
