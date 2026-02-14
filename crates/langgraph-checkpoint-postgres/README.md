# langgraph-checkpoint-postgres

PostgreSQL checkpoint 存储后端，为 [langgraph](../langgraph) 提供生产级的持久化支持。

## 特性

✅ **PostgreSQL 数据库存储** - 基于 SQLx 的类型安全查询
✅ **连接池管理** - 高效的连接复用
✅ **版本控制** - 自动管理多个 checkpoint 版本
✅ **时间点恢复** - 支持恢复到任意历史时间点
✅ **自动清理** - 可配置的版本数量和时间限制
✅ **异步 API** - 完全基于 Tokio 异步运行时
✅ **类型安全** - 编译时检查 SQL 查询

## 安装

在 `Cargo.toml` 中添加：

```toml
[dependencies]
langgraph-checkpoint-postgres = "0.1"
```

## 使用示例

### 基本使用

```rust
use langgraph_checkpoint_postgres::PostgresCheckpointer;
use langgraph::checkpoint::Configuration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 checkpointer
    let checkpointer = PostgresCheckpointer::new(
        "postgresql://user:password@localhost/mydb"
    ).await?;

    // 自动创建表
    checkpointer.migrate().await?;

    // 使用配置
    let config = Configuration {
        thread_id: "conversation-1".to_string(),
        checkpoint_id: None,
    };

    // 保存 checkpoint
    // ...

    Ok(())
}
```

### 配置选项

```rust
use langgraph_checkpoint_postgres::PostgresCheckpointer;

let checkpointer = PostgresCheckpointer::new(database_url).await?
    // 最多保留 100 个版本
    .with_max_versions(100)
    // checkpoint 保留 7 天
    .with_max_age(7 * 24 * 60 * 60);  // 秒
```

### 在 StateGraph 中使用

```rust
use langgraph::state_graph::StateGraph;
use langgraph_checkpoint_postgres::PostgresCheckpointer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 checkpointer
    let checkpointer = PostgresCheckpointer::new(
        "postgresql://user:password@localhost/mydb"
    ).await?;
    checkpointer.migrate().await?;

    // 构建图
    let mut graph = StateGraph::new("start", |state, update| {
        // reducer 逻辑
        state
    });

    // 添加节点...

    // 设置 checkpointer
    let graph = graph.with_checkpointer(checkpointer);

    // 执行图（自动保存 checkpoint）
    let config = Configuration {
        thread_id: "my-thread".to_string(),
        checkpoint_id: None,
    };

    let result = graph.run(initial_state, Some(&config), 100, RunStrategy::StopAtNonLinear).await?;

    Ok(())
}
```

### 高级功能

#### 时间点恢复

```rust
// 恢复到 24 小时前
let timestamp = chrono::Utc::now() - chrono::Duration::days(1);
let checkpoint = checkpointer
    .get_at_time("thread-123", timestamp.timestamp()).await?;
```

#### 列出版本

```rust
// 列出某个线程的所有 checkpoint 版本
let versions = checkpointer
    .list_versions("thread-123", Some(50))
    .await?;

for meta in versions {
    println!("Version {}: {}", meta.version, meta.checkpoint_id);
}
```

#### 清理数据

```rust
// 删除某个线程的所有 checkpoint
let deleted = checkpointer
    .delete_thread("thread-123")
    .await?;

println!("Deleted {} checkpoints", deleted);
```

## 数据库架构

自动创建的表结构：

```sql
CREATE TABLE checkpoints (
    id BIGSERIAL PRIMARY KEY,
    thread_id TEXT NOT NULL,
    checkpoint_id TEXT,
    state JSONB NOT NULL,
    next_nodes JSONB NOT NULL,
    pending_interrupt JSONB,
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 索引
CREATE INDEX idx_checkpoints_thread_id ON checkpoints(thread_id);
CREATE INDEX idx_checkpoints_thread_checkpoint ON checkpoints(thread_id, checkpoint_id);
CREATE INDEX idx_checkpoints_created_at ON checkpoints(created_at);
```

## 配置建议

### 生产环境

```rust
let checkpointer = PostgresCheckpointer::new(
    "postgresql://user:password@production-db.example.com:5432/langgraph?pool_max_size=20"
).await?
.with_max_versions(1000)
.with_max_age(30 * 24 * 60 * 60);  // 30 天
```

### 开发环境

```rust
let checkpointer = PostgresCheckpointer::new(
    "postgresql://postgres:password@localhost:5432/langgraph_dev"
).await?
.with_max_versions(50);  // 保留较少版本
```

## 性能优化

1. **连接池大小**：根据并发量调整 `pool_max_size` 参数
2. **版本控制**：合理设置 `max_versions` 避免数据堆积
3. **索引优化**：已自动创建必要的索引
4. **清理策略**：设置 `max_age` 定期清理过期数据

## 环境变量

推荐使用环境变量管理数据库连接：

```rust
use std::env;

let database_url = env::var("DATABASE_URL")
    .unwrap_or_else(|_| "postgresql://postgres:password@localhost/langgraph".to_string());

let checkpointer = PostgresCheckpointer::new(&database_url).await?;
```

## 依赖要求

- PostgreSQL 12+
- Tokio 异步运行时

## License

MIT OR Apache-2.0
