# Solum 开发入口

工作规则见 [`AGENTS.md`](AGENTS.md)，文档导航见 [`docs/README.md`](docs/README.md)。

修改前先检查工作区，并按任务阅读 `ARCHITECTURE.md`、`PITFALLS.md`；涉及前端再读 `PRODUCT.md`、`DESIGN.md`。完成后运行：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

不要把 HarmonyOS、Android、桌面、主机服务或生产验证结果互相替代。
