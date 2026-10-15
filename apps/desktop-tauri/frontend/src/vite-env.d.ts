/// <reference types="vite/client" />

// 骨架前端会 `import "./styles.css"`（Vite 的 CSS 侧载）。这份引用带来
// `*.css` 等资源模块声明，让 tsc 认这类副作用导入；同时保留 `import.meta.env` 的类型。
// 之所以要显式引用：tsconfig 的 `types: []` 关掉了自动 @types 收集，
// 这样壳的全局类型面只包含显式声明的东西，不随 node_modules 里有什么而漂移。
