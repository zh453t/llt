//! `llt query`：查询已生成的 `llt-timetable.sqlite`。
//!
//! 语义与 GUI 一致（都走 `lltdb`）：日期默认今天，只显示当日开行的车次；
//! 车站/站到站按停站「第几天」倒推始发日期；`--all` 忽略日期。

use std::path::Path;

use anyhow::{Result, bail};
use jiff::civil::Date;
use serde_json::{Value, json};

use lltdb::date::{
    date_window, date_ymd, duration_cn, origin_ymd, parse_date, today, weekday_cn, ymd,
};
use lltdb::extras::{DiningCar, Duty, SameCityGroup, ViaPoint};
use lltdb::{Db, OdResult, StationStop, Stop, TrainSummary};

use crate::cli::{OdArgs, QueryArgs, QueryCommand, QueryOpts, StationArgs, StopsArgs, TrainArgs};
use crate::table::Table;
use lltdata::format::train_no;

/// 站到站查询取候选的上限：先取够，再按日期过滤并自行截断。
const OD_FETCH_LIMIT: usize = 100_000;

/// `query train` 最多为多少趟车展开附件信息（前缀匹配时可能上百趟）。
const MAX_DETAIL_TRAINS: usize = 20;

pub fn run(args: &QueryArgs) -> Result<()> {
    match &args.command {
        QueryCommand::Train(args) => train(args),
        QueryCommand::Stops(args) => stops(args),
        QueryCommand::Station(args) => station(args),
        QueryCommand::Od(args) => od(args),
    }
}

/// 一次查询的公共上下文。
struct Ctx {
    db: Db,
    date: Date,
    ymd: i64,
    filter_by_date: bool,
    limit: Option<usize>,
    json: bool,
    /// 开行周期表：`rule_index -> (period, mask)`，与界面同一份判定。
    rules: std::collections::HashMap<i64, (i64, i64)>,
}

impl Ctx {
    fn new(opts: &QueryOpts) -> Result<Self> {
        let date = match &opts.date {
            Some(text) => parse_date(text).map_err(anyhow::Error::msg)?,
            None => today(),
        };
        let db = open_db(opts.base.db.as_deref(), opts.base.strict, opts.base.quiet)?;
        let rules = db.all_rules();
        Ok(Self {
            db,
            rules,
            date,
            ymd: ymd(date),
            filter_by_date: !opts.all,
            limit: (opts.limit > 0).then_some(opts.limit),
            json: opts.base.json,
        })
    }

    /// 某一天这趟车开不开行：区间内还要过开行周期那一关（与界面一致）。
    fn running(&self, date: i64, from: i64, to: i64, rule_index: Option<i64>) -> bool {
        let (period, mask) = rule_index
            .and_then(|index| self.rules.get(&index).copied())
            .unwrap_or((1, 0));
        lltdb::date::run_state(from, to, date, period, mask) == lltdb::date::RunState::Running
    }

    fn date_label(&self) -> String {
        format!("{} {}", date_ymd(self.ymd), weekday_cn(self.date))
    }

    /// 表头/提示里的范围说明。
    fn scope(&self) -> String {
        if self.filter_by_date {
            format!("{} 开行", self.date_label())
        } else {
            "全部开行区间".to_owned()
        }
    }

    fn json_date(&self) -> Value {
        if self.filter_by_date {
            json!(date_ymd(self.ymd))
        } else {
            Value::Null
        }
    }
}

// ---------------------------------------------------------------- 车次

fn train(args: &TrainArgs) -> Result<()> {
    let ctx = Ctx::new(&args.opts)?;
    let code = args.code.trim();
    let mut rows = select_trains(&ctx.db, code, args.prefix)?;
    if ctx.filter_by_date {
        rows.retain(|train| ctx.running(ctx.ymd, train.date_from, train.date_to, train.rule_index));
    }
    let total = rows.len();
    let shown = truncate(&mut rows, ctx.limit);

    if ctx.json {
        let items: Vec<Value> = rows
            .iter()
            .map(|train| {
                let mut value = train_json(train);
                attach_details(&mut value, &ctx, train);
                value
            })
            .collect();
        return print_json(&json!({
            "query": "train",
            "code": code,
            "date": ctx.json_date(),
            "total": total,
            "shown": shown,
            "truncated": shown < total,
            "items": items,
        }));
    }

    if total == 0 {
        // 车次存在但当天没排上，多半是开行周期（隔日开行等）的停开日，说清楚点。
        let reason = if ctx.filter_by_date && !select_trains(&ctx.db, code, args.prefix)?.is_empty()
        {
            format!("{} 停开（按开行计划这天不开行）", ctx.date_label())
        } else {
            ctx.scope()
        };
        eprintln!("车次「{code}」：{reason}，没有匹配结果");
        return Ok(());
    }

    eprintln!(
        "车次「{code}」· {} · {total} 趟{}",
        ctx.scope(),
        if args.prefix {
            "（前缀匹配）"
        } else {
            ""
        }
    );
    let mut table = Table::new(&["车次", "车型", "起 → 终", "里程", "天数", "开行区间"]);
    for train in &rows {
        table.push(vec![
            train.code.clone(),
            train.kind.clone(),
            format!("{} → {}", train.first, train.last),
            format!("{} km", train.total_km),
            format!("{} 天", train.days),
            date_window(train.date_from, train.date_to),
        ]);
    }
    table.print();
    print_truncated(total, shown);

    // 只有一趟车时展开完整交路 / 经由；多趟时每趟只给一行摘要。
    for train in rows.iter().take(MAX_DETAIL_TRAINS) {
        print_details(&ctx, train, shown == 1);
    }
    if shown > MAX_DETAIL_TRAINS {
        flush_stdout();
        eprintln!("· 只展开了前 {MAX_DETAIL_TRAINS} 趟，可用 --limit 收窄");
    }
    Ok(())
}

// ---------------------------------------------------------------- 停站

fn stops(args: &StopsArgs) -> Result<()> {
    let db = open_db(args.base.db.as_deref(), args.base.strict, args.base.quiet)?;
    let index = resolve_train_index(&db, args.code.as_deref(), args.index)?;
    let detail = db.train_detail(index).map_err(anyhow::Error::msg)?;
    let summary = &detail.summary;

    if args.base.json {
        let items: Vec<Value> = detail.stops.iter().map(stop_json).collect();
        return print_json(&json!({
            "query": "stops",
            "train": summary.code,
            "train_index": summary.index,
            "type": summary.kind,
            "first_station": summary.first,
            "last_station": summary.last,
            "total_km": summary.total_km,
            "days": summary.days,
            "date_from": summary.date_from,
            "date_to": summary.date_to,
            "count": detail.stops.len(),
            "stops": items,
            "duties": duties_json(&db, summary)?,
            "via": via_json(&db, summary.index)?,
            "dining_cars": dining_json(&db, summary.index)?,
        }));
    }

    eprintln!(
        "{}  {} → {} · {} 站 · {} km · {}",
        summary.code,
        summary.first,
        summary.last,
        detail.stops.len(),
        summary.total_km,
        date_window(summary.date_from, summary.date_to),
    );
    // 有车次变化 / 站台数据时才加列，老库的输出保持原样。
    let show_display = detail
        .stops
        .iter()
        .any(|stop| stop.display_train != summary.code);
    let show_platform = detail.stops.iter().any(|stop| !stop.platform.is_empty());
    let mut table = Table::new(&stop_header(show_display, show_platform));
    for (position, stop) in detail.stops.iter().enumerate() {
        let mut row = vec![(position + 1).to_string()];
        if show_display {
            row.push(stop.display_train.clone());
        }
        row.extend([
            stop.station.clone(),
            stop.arrive.clone(),
            stop.depart.clone(),
            stop.dwell_min.to_string(),
            format!("{} km", stop.km),
            format!("第 {} 天", stop.day),
        ]);
        if show_platform {
            row.push(stop.platform.clone());
        }
        table.push(row);
    }
    table.print();
    print_details_for(&db, summary, None, true);
    Ok(())
}

// ---------------------------------------------------------------- 车站

/// 停站表的列：有车次变化 / 站台数据时才加列，老库的输出保持原样。
fn stop_header(display: bool, platform: bool) -> Vec<&'static str> {
    let mut header = vec!["序"];
    if display {
        header.push("车次");
    }
    header.extend(["车站", "到达", "发车", "停留(分)", "里程", "第几天"]);
    if platform {
        header.push("站台");
    }
    header
}

fn station(args: &StationArgs) -> Result<()> {
    let ctx = Ctx::new(&args.opts)?;
    let name = resolve_station(&ctx.db, &args.name)?;
    let mut rows = ctx.db.station_board(&name).map_err(anyhow::Error::msg)?;
    if ctx.filter_by_date {
        rows.retain(|stop| {
            ctx.running(
                origin_ymd(ctx.date, stop.day),
                stop.date_from,
                stop.date_to,
                stop.rule_index,
            )
        });
    }
    let total = rows.len();
    let shown = truncate(&mut rows, ctx.limit);

    let same_city = same_city_json(&ctx.db, &name);

    if ctx.json {
        let items: Vec<Value> = rows.iter().map(station_json).collect();
        let mut value = json!({
            "query": "station",
            "station": name,
            "date": ctx.json_date(),
            "total": total,
            "shown": shown,
            "truncated": shown < total,
            "items": items,
        });
        if let (Some(object), Some(city)) = (value.as_object_mut(), same_city) {
            object.insert("same_city".to_owned(), city);
        }
        return print_json(&value);
    }

    if total == 0 {
        eprintln!("车站「{name}」：{}，没有经停列车", ctx.scope());
        return Ok(());
    }

    eprintln!("车站「{name}」· {} · {total} 趟", ctx.scope());
    let mut table = Table::new(&[
        "车次",
        "车型",
        "到达",
        "发车",
        "停留(分)",
        "第几天",
        "起 → 终",
        "里程",
    ]);
    for stop in &rows {
        table.push(vec![
            stop.code.clone(),
            stop.kind.clone(),
            stop.arrive.clone(),
            stop.depart.clone(),
            stop.dwell_min.to_string(),
            format!("第 {} 天", stop.day),
            format!("{} → {}", stop.first, stop.last),
            format!("{} km", stop.km),
        ]);
    }
    table.print();
    print_truncated(total, shown);
    print_same_city(&ctx.db, &name);
    Ok(())
}

// ---------------------------------------------------------------- 站到站

fn od(args: &OdArgs) -> Result<()> {
    let ctx = Ctx::new(&args.opts)?;
    let from = resolve_station(&ctx.db, &args.from)?;
    let to = resolve_station(&ctx.db, &args.to)?;
    let (mut rows, candidate_overflow) = ctx
        .db
        .od_search(&from, &to, OD_FETCH_LIMIT)
        .map_err(anyhow::Error::msg)?;
    if candidate_overflow {
        eprintln!("注意：{from} → {to} 的候选超过 {OD_FETCH_LIMIT} 条，结果可能不完整");
    }
    if ctx.filter_by_date {
        rows.retain(|hit| {
            ctx.running(
                origin_ymd(ctx.date, hit.depart_day),
                hit.date_from,
                hit.date_to,
                hit.rule_index,
            )
        });
    }
    let total = rows.len();
    let shown = truncate(&mut rows, ctx.limit);

    if ctx.json {
        let items: Vec<Value> = rows.iter().map(od_json).collect();
        return print_json(&json!({
            "query": "od",
            "from": from,
            "to": to,
            "date": ctx.json_date(),
            "total": total,
            "shown": shown,
            "truncated": shown < total,
            "items": items,
        }));
    }

    if total == 0 {
        eprintln!("{from} → {to}：{}，没有直达车次", ctx.scope());
        return Ok(());
    }

    eprintln!("{from} → {to} · {} · {total} 趟直达", ctx.scope());
    let mut table = Table::new(&["车次", "车型", "出发", "到达", "历时", "里程", "起 → 终"]);
    for hit in &rows {
        table.push(vec![
            hit.code.clone(),
            hit.kind.clone(),
            time_with_day(&hit.depart, hit.depart_day),
            time_with_day(&hit.arrive, hit.arrive_day),
            duration_cn(hit.duration_min()),
            format!("{} km", hit.km()),
            format!("{} → {}", hit.first, hit.last),
        ]);
    }
    table.print();
    print_truncated(total, shown);
    Ok(())
}

// ---------------------------------------------------------------- 同城车站

/// 同城组：「模糊站名」勾选后按站搜索给出的候选。
///
/// 数据里有互为镜像的组（天津的 `192(滨海)` 与 `2310(天津)`），
/// 所以要把所有相关组的成员并起来。
fn same_city_names(db: &Db, station: &str) -> Vec<String> {
    let Some(index) = db.station_index(station).ok().flatten() else {
        return Vec::new();
    };
    let groups = db.same_city(index).unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for group in &groups {
        for member in &group.members {
            if member.station_index.is_some() && !names.contains(&member.station) {
                names.push(member.station.clone());
            }
        }
    }
    // 自身排最前
    if let Some(position) = names.iter().position(|name| name == station) {
        let own = names.remove(position);
        names.insert(0, own);
    }
    names
}

fn same_city_json(db: &Db, station: &str) -> Option<Value> {
    let index = db.station_index(station).ok().flatten()?;
    let groups = db.same_city(index).ok()?;
    if groups.is_empty() {
        return None;
    }
    let groups: Vec<Value> = groups.iter().map(group_json).collect();
    let names = same_city_names(db, station);
    Some(json!({
        "station_index": index,
        "stations": names,
        "groups": groups,
    }))
}

fn group_json(group: &SameCityGroup) -> Value {
    let members: Vec<Value> = group
        .members
        .iter()
        .map(|member| {
            json!({
                "station_index": member.station_index,
                "station": member.station,
                "is_group_head": member.is_group_head,
            })
        })
        .collect();
    json!({
        "group_id": group.group_id,
        "group_name": group.group_name,
        "is_city_node": group.is_city_node,
        "members": members,
    })
}

/// 打印同城车站与电报码（没有则静默跳过）。
fn print_same_city(db: &Db, station: &str) {
    let names = same_city_names(db, station);
    let tele = db.tele_code(station).ok().flatten();
    if names.len() <= 1 && tele.is_none() {
        return;
    }
    flush_stdout();
    if names.len() > 1 {
        eprintln!("同城车站（{} 站）：{}", names.len(), names.join("、"));
    }
    if let Some(code) = tele {
        eprintln!("电报码：{code}");
    }
}

// ---------------------------------------------------------------- 交路 / 经由

/// 一趟车的附加信息。
struct Details {
    duties: Vec<Duty>,
    via: Vec<ViaPoint>,
    dining: Vec<DiningCar>,
}

impl Details {
    fn is_empty(&self) -> bool {
        self.duties.is_empty() && self.via.is_empty() && self.dining.is_empty()
    }
}

/// 读取交路 / 经由 / 餐车；`date` 有值时优先只保留当日有效的记录。
fn details_for(db: &Db, train: &TrainSummary, date: Option<i64>) -> Details {
    let mut duties = db.duties_for(train.index).unwrap_or_default();
    let mut dining = db.dining_cars(train.index).unwrap_or_default();
    let via = db.via_points(train.index).unwrap_or_default();

    if let Some(date) = date {
        let valid: Vec<Duty> = duties
            .iter()
            .filter(|duty| duty.runs_on(date))
            .cloned()
            .collect();
        if !valid.is_empty() {
            duties = valid;
        }
        let valid: Vec<DiningCar> = dining
            .iter()
            .filter(|car| car.runs_on(date))
            .cloned()
            .collect();
        if !valid.is_empty() {
            dining = valid;
        }
    }
    Details {
        duties,
        via,
        dining,
    }
}

/// 交路串渲染：`G1806/7 → [G1] → G1956/7`，方括号表示当前车次。
fn chain_line(duty: &Duty) -> String {
    duty.segments
        .iter()
        .map(|segment| {
            if segment.current {
                format!("[{}]", segment.label)
            } else {
                segment.label.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

fn print_details(ctx: &Ctx, train: &TrainSummary, full: bool) {
    let date = ctx.filter_by_date.then_some(ctx.ymd);
    print_details_for(&ctx.db, train, date, full);
}

/// 打印交路与经由（数据缺失时静默跳过）。
///
/// `full = false` 时只输出两行摘要，用于一次匹配到多趟车的场景。
fn print_details_for(db: &Db, train: &TrainSummary, date: Option<i64>, full: bool) {
    let details = details_for(db, train, date);
    if details.is_empty() {
        return;
    }
    // 表格已经写到 stdout，先把缓冲刷掉，保证提示和表格的顺序正确。
    flush_stdout();

    if !full {
        if let Some(duty) = details.duties.first() {
            eprintln!(
                "  {} 交路：{}（{}）",
                train.code,
                chain_line(duty),
                duty.company
            );
        }
        if let Some(last) = details.via.last() {
            let others = details.via.iter().filter(|point| !point.is_station).count();
            let note = if others > 0 {
                format!("，其中 {others} 个不是车站")
            } else {
                String::new()
            };
            eprintln!(
                "  {} 经由：{} 点 · {} km{note}（llt query stops {} 查看完整经由）",
                train.code,
                details.via.len(),
                last.km,
                train.code
            );
        }
        return;
    }

    for (position, duty) in details.duties.iter().enumerate() {
        let heading = if details.duties.len() > 1 {
            format!("交路 {} / {}", position + 1, details.duties.len())
        } else {
            "交路".to_owned()
        };
        let mut meta = vec![duty.company.clone()];
        if !duty.formation.is_empty() {
            meta.push(format!("编组 {}", duty.formation));
        }
        eprintln!(
            "{} · {} · {} · {}",
            heading,
            train.code,
            date_window(duty.start_date, duty.end_date),
            meta.join(" · ")
        );
        if position == 0 && !details.dining.is_empty() {
            let cars: Vec<String> = details
                .dining
                .iter()
                .map(|car| {
                    if details.dining.len() > 1 {
                        format!(
                            "{}（{}）",
                            car.coach,
                            date_window(car.start_date, car.end_date)
                        )
                    } else {
                        car.coach.clone()
                    }
                })
                .collect();
            eprintln!("  餐车：{}", cars.join("、"));
        }
        println!("  {}", chain_line(duty));
        flush_stdout();
    }

    if !details.via.is_empty() {
        let total = details.via.last().map(|point| point.km).unwrap_or(0);
        eprintln!(
            "经由 · {} · {} 点 · {} km",
            train.code,
            details.via.len(),
            total
        );
        let mut table = Table::new(&["序", "车站", "类型", "线路", "里程"]);
        for point in &details.via {
            table.push(vec![
                point.seq.to_string(),
                point.station.clone(),
                if point.is_station {
                    "车站"
                } else {
                    "非车站"
                }
                .to_owned(),
                point.line.clone(),
                format!("{} km", point.km),
            ]);
        }
        table.print();
    }
}

fn duties_json(db: &Db, train: &TrainSummary) -> Result<Value> {
    let duties = db.duties_for(train.index)?;
    Ok(Value::Array(duties.iter().map(duty_json).collect()))
}

fn duty_json(duty: &Duty) -> Value {
    let segments: Vec<Value> = duty
        .segments
        .iter()
        .map(|segment| {
            json!({
                "seq": segment.seq,
                "label": segment.label,
                "code": segment.code,
                "current": segment.current,
            })
        })
        .collect();
    json!({
        "duty_id": duty.id,
        "start_date": duty.start_date,
        "end_date": duty.end_date,
        "company": duty.company,
        "formation": duty.formation,
        "chain": duty.chain,
        "segments": segments,
    })
}

fn via_json(db: &Db, train_index: i64) -> Result<Value> {
    let points: Vec<Value> = db
        .via_points(train_index)?
        .into_iter()
        .map(|point| {
            json!({
                "seq": point.seq,
                "station": point.station,
                "line": point.line,
                "km": point.km,
                "is_station": point.is_station,
                "station_index": point.station_index,
            })
        })
        .collect();
    Ok(Value::Array(points))
}

fn dining_json(db: &Db, train_index: i64) -> Result<Value> {
    let cars: Vec<Value> = db
        .dining_cars(train_index)?
        .into_iter()
        .map(|car| {
            json!({
                "start_date": car.start_date,
                "end_date": car.end_date,
                "coach": car.coach,
            })
        })
        .collect();
    Ok(Value::Array(cars))
}

/// 把交路 / 经由 / 餐车挂到 `query train --json` 的条目上。
fn attach_details(value: &mut Value, ctx: &Ctx, train: &TrainSummary) {
    let date = ctx.filter_by_date.then_some(ctx.ymd);
    let details = details_for(&ctx.db, train, date);
    let duties: Vec<Value> = details.duties.iter().map(duty_json).collect();
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert("duties".to_owned(), Value::Array(duties));
    object.insert(
        "via".to_owned(),
        via_json(&ctx.db, train.index).unwrap_or(Value::Array(Vec::new())),
    );
    object.insert(
        "dining_cars".to_owned(),
        dining_json(&ctx.db, train.index).unwrap_or(Value::Array(Vec::new())),
    );
}

// ---------------------------------------------------------------- 共用

fn open_db(explicit: Option<&Path>, strict: bool, quiet: bool) -> Result<Db> {
    let located = lltdb::path::find(explicit, strict).map_err(|err| anyhow::anyhow!("{err}"))?;
    if !quiet {
        // 隐式搜索有可能换库，所以把命中的路径和来源打出来。
        eprintln!(
            "数据库：{}（{}）",
            located.path.display(),
            located.source.describe()
        );
    }
    Db::open(&located.path).map_err(anyhow::Error::msg)
}

fn resolve_station(db: &Db, input: &str) -> Result<String> {
    let name = input.trim();
    if name.is_empty() {
        bail!("站名不能为空");
    }
    if let Some(found) = db.find_station(name).map_err(anyhow::Error::msg)? {
        return Ok(found);
    }

    // `s.i` 里没有「成都」这类站（站改中），但 `sn.dat` 有城市名节点。
    if let Ok(groups) = db.city_groups(name)
        && let Some(group) = groups.first()
    {
        let stations: Vec<&str> = group
            .members
            .iter()
            .filter(|member| member.station_index.is_some())
            .map(|member| member.station.as_str())
            .collect();
        if !stations.is_empty() {
            bail!(
                "「{name}」是城市名，不是车站；可用：{}",
                stations.join("、")
            );
        }
    }

    let suggestions = db
        .station_suggestions(name, 8)
        .map_err(anyhow::Error::msg)?;
    if suggestions.is_empty() {
        bail!("未找到车站「{name}」");
    }
    bail!("未找到车站「{name}」，相近的有：{}", suggestions.join("、"))
}

/// `query train` 的匹配规则：优先精确车次，其次精确车次号，最后才是前缀。
///
/// 这样 `llt query train G1` 只查 G1（而不是 G1/G10/G100…），
/// `llt query train 54` 只查车次号为 54 的几趟；要看前缀请加 `--prefix`。
fn select_trains(db: &Db, code: &str, prefix: bool) -> Result<Vec<TrainSummary>> {
    let matches = db.search_trains(code, None).map_err(anyhow::Error::msg)?;
    if prefix || code.is_empty() {
        return Ok(matches);
    }

    let exact: Vec<TrainSummary> = matches
        .iter()
        .filter(|train| train.code.eq_ignore_ascii_case(code))
        .cloned()
        .collect();
    if !exact.is_empty() {
        return Ok(exact);
    }

    let by_number: Vec<TrainSummary> = matches
        .iter()
        .filter(|train| train_no(&train.code) == code)
        .cloned()
        .collect();
    if !by_number.is_empty() {
        return Ok(by_number);
    }
    Ok(matches)
}

/// 把车次（或车次号）解析成唯一的 `train_index`。
fn resolve_train_index(db: &Db, code: Option<&str>, index: Option<i64>) -> Result<i64> {
    if let Some(index) = index {
        return Ok(index);
    }
    let Some(code) = code.map(str::trim).filter(|code| !code.is_empty()) else {
        bail!("需要车次，或用 --index <N> 指定");
    };

    // 与 `query train` 相同的匹配规则：精确车次 > 精确车次号 > 前缀。
    let matches = select_trains(db, code, false)?;
    if matches.is_empty() {
        bail!("未找到车次「{code}」，试试去掉前缀的车次号（如 54）");
    }
    if matches.len() == 1 {
        return Ok(matches[0].index);
    }

    let mut candidates = String::new();
    for train in matches.iter().take(10) {
        candidates.push_str(&format!(
            "\n  [{}] {:<8} {} → {}  {}",
            train.index,
            train.code,
            train.first,
            train.last,
            date_window(train.date_from, train.date_to),
        ));
    }
    if matches.len() > 10 {
        candidates.push_str("\n  …");
    }
    bail!(
        "车次「{code}」匹配到 {} 趟，请写完整车次或用 --index 指定：{candidates}",
        matches.len()
    )
}

fn truncate<T>(rows: &mut Vec<T>, limit: Option<usize>) -> usize {
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    rows.len()
}

fn print_truncated(total: usize, shown: usize) {
    // stdout 是块缓冲、stderr 不是；重定向到同一文件时先 flush 以免提示插进表格。
    flush_stdout();
    if shown < total {
        eprintln!("共 {total} 条，只显示前 {shown} 条（--limit 0 显示全部）");
    }
}

fn flush_stdout() {
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn time_with_day(time: &str, day: i64) -> String {
    if day > 1 {
        format!("{time}+{}天", day - 1)
    } else {
        time.to_owned()
    }
}

// ---------------------------------------------------------------- JSON

fn train_json(train: &TrainSummary) -> Value {
    json!({
        "train_index": train.index,
        "train": train.code,
        "type": train.kind,
        "first_station": train.first,
        "last_station": train.last,
        "total_km": train.total_km,
        "days": train.days,
        "date_from": train.date_from,
        "date_to": train.date_to,
    })
}

fn stop_json(stop: &Stop) -> Value {
    json!({
        "seq": stop.seq,
        "station": stop.station,
        "display_train": stop.display_train,
        "platform": stop.platform,
        "arrive": stop.arrive,
        "depart": stop.depart,
        "dwell_min": stop.dwell_min,
        "km": stop.km,
        "day": stop.day,
        "arrive_min": stop.arrive_min,
        "depart_min": stop.depart_min,
    })
}

fn station_json(stop: &StationStop) -> Value {
    json!({
        "station": stop.station,
        "train_index": stop.train_index,
        "train": stop.code,
        "type": stop.kind,
        "first_station": stop.first,
        "last_station": stop.last,
        "arrive": stop.arrive,
        "depart": stop.depart,
        "dwell_min": stop.dwell_min,
        "day": stop.day,
        "km": stop.km,
        "date_from": stop.date_from,
        "date_to": stop.date_to,
    })
}

fn od_json(hit: &OdResult) -> Value {
    json!({
        "train_index": hit.train_index,
        "train": hit.code,
        "type": hit.kind,
        "first_station": hit.first,
        "last_station": hit.last,
        "from_station": hit.from_station,
        "to_station": hit.to_station,
        "depart": hit.depart,
        "depart_min": hit.depart_min,
        "depart_day": hit.depart_day,
        "arrive": hit.arrive,
        "arrive_min": hit.arrive_min,
        "arrive_day": hit.arrive_day,
        "duration_min": hit.duration_min(),
        "from_km": hit.from_km,
        "to_km": hit.to_km,
        "km": hit.km(),
        "date_from": hit.date_from,
        "date_to": hit.date_to,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture_db() -> Option<Db> {
        // 默认用仓库里的库；LLT_TEST_DB 可指向任意 `llt build` 产物。
        if let Some(db) = std::env::var_os("LLT_TEST_DB") {
            let path = PathBuf::from(db);
            return path.is_file().then(|| Db::open(&path).unwrap());
        }
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/timetable/llt-timetable.sqlite");
        path.exists().then(|| Db::open(&path).unwrap())
    }

    #[test]
    fn resolves_exact_train_code() {
        let Some(db) = fixture_db() else {
            return;
        };
        let expected = db
            .search_trains("G1", None)
            .unwrap()
            .into_iter()
            .find(|train| train.code == "G1")
            .unwrap()
            .index;
        assert_eq!(
            resolve_train_index(&db, Some("G1"), None).unwrap(),
            expected
        );
        assert_eq!(resolve_train_index(&db, None, Some(42)).unwrap(), 42);
    }

    #[test]
    fn ambiguous_train_numbers_ask_for_index() {
        let Some(db) = fixture_db() else {
            return;
        };
        let err = resolve_train_index(&db, Some("54"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--index"), "{err}");
        assert!(err.contains("K54"), "{err}");
    }

    #[test]
    fn unknown_train_reports_hint() {
        let Some(db) = fixture_db() else {
            return;
        };
        let err = resolve_train_index(&db, Some("Q9999"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("未找到车次"), "{err}");
    }

    #[test]
    fn station_resolution_uses_exact_match_then_suggestions() {
        let Some(db) = fixture_db() else {
            return;
        };
        assert_eq!(resolve_station(&db, "北京").unwrap(), "北京");
        // 模糊输入会给出相近站名
        let err = resolve_station(&db, "京南").unwrap_err().to_string();
        assert!(err.contains("相近") && err.contains("北京南"), "{err}");
        assert!(resolve_station(&db, "不存在的车站").is_err());
    }

    #[test]
    fn details_include_duty_and_via() {
        let Some(db) = fixture_db() else {
            return;
        };
        let Some(train) = db
            .search_trains("G1", None)
            .unwrap()
            .into_iter()
            .find(|train| train.code == "G1")
        else {
            return;
        };
        let details = details_for(&db, &train, None);
        if details.is_empty() {
            return; // 旧库没有附加表
        }
        // 交路里应能定位到本车次，并渲染成 [G1]。
        let duty = details.duties.first().expect("G1 的交路");
        let chain = chain_line(duty);
        assert!(chain.contains("[G1]"), "{chain}");
        // 经由首尾与里程应与停站表一致。
        assert!(!details.via.is_empty());
        assert_eq!(details.via[0].station, train.first);
        assert_eq!(details.via.last().unwrap().station, train.last);
        assert_eq!(details.via.last().unwrap().km, train.total_km);
    }

    #[test]
    fn same_city_candidates_are_merged() {
        let Some(db) = fixture_db() else {
            return;
        };
        let names = same_city_names(&db, "北京");
        if names.is_empty() {
            return; // 旧库没有同城表
        }
        assert_eq!(names.len(), 11, "{names:?}");
        assert_eq!(names[0], "北京", "自身应排最前");

        // 天津 有镜像组，并集 12 站。
        let tianjin = same_city_names(&db, "天津");
        assert_eq!(tianjin.len(), 12, "{tianjin:?}");
        assert_eq!(tianjin[0], "天津");
    }

    #[test]
    fn city_name_suggests_its_stations() {
        let Some(db) = fixture_db() else {
            return;
        };
        if db.city_groups("成都").unwrap().is_empty() {
            return;
        }
        let err = resolve_station(&db, "成都").unwrap_err().to_string();
        assert!(err.contains("城市名"), "{err}");
        assert!(err.contains("成都东") && err.contains("犀浦"), "{err}");
    }

    #[test]
    fn explicit_db_path_must_exist() {
        let err = lltdb::path::find(Some(Path::new("no-such-file.sqlite")), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("找不到"), "{err}");
        assert!(
            err.contains("no-such-file.sqlite"),
            "错误里应列出尝试过的路径：{err}"
        );
    }
}
