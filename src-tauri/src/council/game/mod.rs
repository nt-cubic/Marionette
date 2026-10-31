//! 身份局游戏(简化三国杀 v1):engine(纯逻辑)+ player(私有视角
//! 决策代理)+ runner(对局循环)。调研依据与规则简化清单见
//! docs/council/formation.md §6。

pub mod engine;
pub mod player;
pub mod runner;
