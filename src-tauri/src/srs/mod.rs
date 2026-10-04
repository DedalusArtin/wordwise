//! 记忆调度引擎（间隔重复 / 遗忘曲线）。
//!
//! 算法基于 SM-2 改良版：
//! - 以 `base_intervals` 作为记忆周期表的骨架（需求 4），
//!   每次连续答对就沿周期表前进一格，答错则回退并按 lapse 处理。
//! - `ease_factor` 会根据答题表现动态调整，实现「难度自适应」。
//! - 同一个词在短期内的反复遗忘会被识别为 leech（常错词）并纳入强化记忆（需求 5）。

use crate::models::{SrsConfig, StudyState};
use chrono::{Datelike, Local, TimeZone};

/// 一次作答的质量。借鉴 SM-2 的 0-5 评分，这里简化为三档，
/// 并额外区分「用时过长但答对」的情况。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// 答错
    Wrong,
    /// 答对但很勉强（用时长 / 犹豫）
    Hard,
    /// 答对，正常
    Good,
    /// 答对且非常轻松
    Easy,
}

impl Grade {
    /// 映射到 SM-2 的 0-5 分。
    ///
    /// SM-2 中「质量 < 3」视为遗忘，需要重新学习；≥3 视为成功回忆。
    pub fn quality(self) -> f64 {
        match self {
            Grade::Wrong => 2.0,
            Grade::Hard => 3.0,
            Grade::Good => 4.0,
            Grade::Easy => 5.0,
        }
    }

    /// 是否算作「答对」。沿用 SM-2 的判定：质量分 ≥ 3 即通过。
    pub fn is_correct(self) -> bool {
        self.quality() >= 3.0
    }
}

/// 调度结果，供前端展示「下次复习时间」。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScheduleResult {
    /// 更新后的状态
    pub state: StudyState,
    /// 下次复习的间隔（天）
    pub interval_days: f64,
    /// 下次复习时间（Unix 秒）
    pub due_at: i64,
    /// 人类可读的下次复习时间
    pub due_text: String,
    /// 本次是否被标记为强化记忆
    pub became_leech: bool,
    /// 是否刚刚掌握
    pub became_mastered: bool,
}

/// 将 Unix 秒格式化为本地时间字符串。
pub fn format_ts(ts: i64) -> String {
    match Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        _ => "-".to_string(),
    }
}

/// 把秒数差异转成「刚刚 / 3 小时后 / 2 天后」这样的文案。
pub fn humanize_due(due_at: i64, now: i64) -> String {
    let diff = due_at - now;
    if diff <= 0 {
        return "现在".to_string();
    }
    let mins = diff / 60;
    if mins < 60 {
        format!("{} 分钟后", mins.max(1))
    } else if mins < 60 * 24 {
        format!("{} 小时后", mins / 60)
    } else {
        format!("{} 天后", mins / (60 * 24))
    }
}

/// 核心调度函数：根据作答质量推进记忆周期。
pub fn schedule(state: &mut StudyState, grade: Grade, cfg: &SrsConfig, now: i64) -> ScheduleResult {
    let was_leech = state.is_leech;
    let was_mastered = state.is_mastered;

    state.last_review_at = now;

    if grade.is_correct() {
        state.correct_count += 1;

        // 沿记忆周期表前进
        state.repetitions += 1;
        let idx = (state.repetitions - 1).max(0) as usize;
        let base = cfg
            .base_intervals
            .get(idx)
            .copied()
            .unwrap_or_else(|| {
                // 超出周期表后按几何增长
                let last = cfg.base_intervals.last().copied().unwrap_or(30.0);
                last * 1.8_f64.powi((idx as i32) - (cfg.base_intervals.len() as i32) + 1)
            });

        // 难度系数参与修正：难 → 间隔缩短，易 → 间隔拉长
        let ease_adj = state.ease_factor / cfg.initial_ease;
        let mut interval = base * ease_adj;
        match grade {
            Grade::Hard => interval *= 0.6,
            Grade::Easy => interval *= 1.3,
            _ => {}
        }

        // 周期表里的最小间隔不低于半天，避免刚学完又立刻出现
        interval = interval.max(0.5);
        state.interval_days = interval;

        // 更新难度系数（遵循 SM-2：只有「答错」才降 EF，
        // 「勉强答对」只影响本次间隔，不惩罚长期难度，避免好词被越判越难）
        match grade {
            Grade::Easy => state.ease_factor += cfg.ease_bonus,
            Grade::Good | Grade::Hard => {}
            Grade::Wrong => unreachable!(),
        }
        state.ease_factor = state.ease_factor.clamp(cfg.min_ease, 3.2);
    } else {
        // 答错：重置连击，回到短期强化
        state.wrong_count += 1;
        state.repetitions = 0;
        state.interval_days = cfg.lapse_interval;
        state.ease_factor = (state.ease_factor - cfg.ease_penalty).max(cfg.min_ease);
        // 答错的词短时间内必须重现
        state.due_at = now;
    }

    if grade.is_correct() {
        state.due_at = now + (state.interval_days * 86400.0) as i64;
    } else {
        state.due_at = now + (cfg.lapse_interval * 86400.0) as i64;
    }

    state.recompute_mastery();

    // 掌握判定：连续答对达到阈值、间隔已拉到长期档位、且错误率足够低。
    // （仅看 repetitions 会把「学了就忘」的词误判为掌握，故叠加间隔与正确率条件）
    let total_answers = state.correct_count + state.wrong_count;
    let long_interval = state.interval_days >= 30.0;
    let low_error = total_answers == 0 || state.error_rate() <= 0.2;
    let became_mastered = state.repetitions >= cfg.mastered_repetitions
        && long_interval
        && low_error;
    state.is_mastered = became_mastered || was_mastered;

    // 常错词判定（需求 5）：错误率高 + 作答次数够，自动纳入强化记忆
    let total = state.correct_count + state.wrong_count;
    let became_leech = total >= cfg.leech_min_reviews
        && state.error_rate() >= cfg.leech_error_rate
        && !state.is_mastered;
    if became_leech {
        state.is_leech = true;
        // 强化记忆的词，间隔压缩，强制高频重现
        state.interval_days = state.interval_days.min(1.0);
        state.due_at = now + (state.interval_days * 86400.0) as i64;
    }
    // 掌握后自动移出强化队列
    if state.is_mastered && state.is_leech {
        state.is_leech = false;
    }

    ScheduleResult {
        state: state.clone(),
        interval_days: state.interval_days,
        due_at: state.due_at,
        due_text: format_ts(state.due_at),
        became_leech: became_leech && !was_leech,
        became_mastered: state.is_mastered && !was_mastered,
    }
}

/// 计算某天的「记忆强度衰减」，用于进度可视化。
///
/// 返回 0.0-1.0，1 表示记忆最牢固。
pub fn retention(state: &StudyState, now: i64) -> f64 {
    if state.last_review_at == 0 || state.repetitions == 0 {
        return 0.0;
    }
    let elapsed_days = ((now - state.last_review_at) as f64 / 86400.0).max(0.0);
    let stability = (state.interval_days.max(0.5)) * state.ease_factor;
    // 指数遗忘曲线 R = e^(-t/S)
    (-elapsed_days / stability).exp().clamp(0.0, 1.0)
}

/// 生成未来 N 天的复习计划，供「复习计划」视图使用（需求 4）。
pub fn build_plan(states: &[StudyState], days: i64, now: i64) -> Vec<PlanDay> {
    let today = Local.timestamp_opt(now, 0).single();
    let today = match today {
        Some(d) => d.date_naive(),
        None => return Vec::new(),
    };

    let mut out = Vec::new();
    for offset in 0..days {
        let date = today + chrono::Duration::days(offset);
        let start = Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .map(|d| d.timestamp())
            .unwrap_or(now);
        let end = start + 86400;

        let count = states
            .iter()
            .filter(|s| !s.is_mastered && s.due_at >= start && s.due_at < end)
            .count() as i64;
        let overdue = if offset == 0 {
            states
                .iter()
                .filter(|s| !s.is_mastered && s.due_at < now)
                .count() as i64
        } else {
            0
        };

        out.push(PlanDay {
            date: date.format("%Y-%m-%d").to_string(),
            weekday: weekday_cn(date.weekday()),
            count,
            overdue,
            is_today: offset == 0,
        });
    }
    out
}

fn weekday_cn(w: chrono::Weekday) -> String {
    match w {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    }
    .to_string()
}

/// 复习计划中的一天。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlanDay {
    pub date: String,
    pub weekday: String,
    pub count: i64,
    pub overdue: i64,
    pub is_today: bool,
}

/// 判断今天是否已经算过「连续学习天数」。
pub fn day_key(ts: i64) -> String {
    match Local.timestamp_opt(ts, 0).single() {
        Some(d) => d.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StudyState;

    fn cfg() -> SrsConfig {
        SrsConfig::default()
    }

    #[test]
    fn correct_answer_advances_interval() {
        let c = cfg();
        let now = 1_700_000_000;
        let mut st = StudyState::new("apple", "en", now);
        let r1 = schedule(&mut st, Grade::Good, &c, now);
        assert!(r1.interval_days > 0.0);
        let r2 = schedule(&mut st, Grade::Good, &c, now + 86400);
        // 第二次间隔应大于第一次（沿周期表前进）
        assert!(r2.interval_days > r1.interval_days);
    }

    #[test]
    fn wrong_answer_resets_repetitions() {
        let c = cfg();
        let now = 1_700_000_000;
        let mut st = StudyState::new("apple", "en", now);
        schedule(&mut st, Grade::Good, &c, now);
        schedule(&mut st, Grade::Good, &c, now + 86400);
        assert_eq!(st.repetitions, 2);
        let r = schedule(&mut st, Grade::Wrong, &c, now + 2 * 86400);
        assert_eq!(st.repetitions, 0);
        assert!(r.state.ease_factor < c.initial_ease);
    }

    #[test]
    fn repeated_errors_become_leech() {
        let c = cfg();
        let now = 1_700_000_000;
        let mut st = StudyState::new("hard", "en", now);
        for i in 0..5 {
            schedule(&mut st, Grade::Wrong, &c, now + i * 1000);
        }
        assert!(st.is_leech, "连续答错应进入强化记忆");
    }

    #[test]
    fn retention_decays_over_time() {
        let c = cfg();
        let now = 1_700_000_000;
        let mut st = StudyState::new("apple", "en", now);
        schedule(&mut st, Grade::Good, &c, now);
        let r_now = retention(&st, now);
        let r_later = retention(&st, now + 30 * 86400);
        assert!(r_now > r_later, "时间越久记忆强度应越低");
    }

    #[test]
    fn plan_has_requested_days() {
        let now = 1_700_000_000;
        let states = vec![StudyState::new("a", "en", now)];
        let plan = build_plan(&states, 7, now);
        assert_eq!(plan.len(), 7);
        assert!(plan[0].is_today);
    }
}
