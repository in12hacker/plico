#!/usr/bin/env python3
"""Minimal OpenAI-compatible embedding server using sentence-transformers."""

import argparse
import json
from typing import Any

from sentence_transformers import SentenceTransformer


def run_server(model_name: str = "BAAI/bge-m3", port: int = 18922, device: str = "cpu"):
    print(f"Loading {model_name} ...")
    model = SentenceTransformer(model_name, device=device)
    dims = model.get_sentence_embedding_dimension()
    print(f"Model loaded. Dimensions: {dims}")

    try:
        from fastapi import FastAPI, Request
        from uvicorn import run

        app = FastAPI()

        @app.post("/v1/embeddings")
        async def embeddings(request: Request):
            body = await request.json()
            inputs = body.get("input", [])
            if isinstance(inputs, str):
                inputs = [inputs]
            vectors = model.encode(inputs, normalize_embeddings=True).tolist()
            return {
                "object": "list",
                "data": [
                    {"object": "embedding", "embedding": v, "index": i}
                    for i, v in enumerate(vectors)
                ],
                "model": model_name,
                "usage": {"prompt_tokens": sum(len(t.split()) for t in inputs), "total_tokens": 0},
            }

        @app.get("/v1/models")
        async def models():
            return {
                "object": "list",
                "data": [{"id": model_name, "object": "model"}],
            }

        run(app, host="0.0.0.0", port=port, log_level="warning")
    except ImportError:
        # Fallback to stdlib http.server
        from http.server import BaseHTTPRequestHandler, HTTPServer

        class Handler(BaseHTTPRequestHandler):
            def do_post(self):
                content_len = int(self.headers.get("Content-Length", 0))
                body = json.loads(self.rfile.read(content_len))
                inputs = body.get("input", [])
                if isinstance(inputs, str):
                    inputs = [inputs]
                vectors = model.encode(inputs, normalize_embeddings=True).tolist()
                resp = {
                    "object": "list",
                    "data": [
                        {"object": "embedding", "embedding": v, "index": i}
                        for i, v in enumerate(vectors)
                    ],
                    "model": model_name,
                }
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(resp).encode())

            def do_get(self):
                resp = {"object": "list", "data": [{"id": model_name, "object": "model"}]}
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(resp).encode())

            def log_message(self, format, *args):
                pass

        server = HTTPServer(("0.0.0.0", port), Handler)
        print(f"Serving on http://0.0.0.0:{port}/v1/embeddings")
        server.serve_forever()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="BAAI/bge-m3")
    parser.add_argument("--port", type=int, default=18922)
    parser.add_argument("--device", default="cpu")
    args = parser.parse_args()
    run_server(args.model, args.port, args.device)
