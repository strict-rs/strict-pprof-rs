// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::Hash;

use spin::RwLock;

use crate::Error;
use crate::Result;
use crate::frames::Frames;
use crate::frames::UnresolvedFrames;
use crate::profiler::Profiler;
use crate::timer::ReportTiming;

/// The final presentation of a report which is actually an `HashMap` from `Frames` to isize
/// (count).
pub struct Report {
  /// Key is a backtrace captured by profiler and value is count of it.
  pub data: HashMap<Frames, isize>,

  /// Collection frequency, start time, duration.
  pub timing: ReportTiming,
}

/// The presentation of an unsymbolicated report which is actually an `HashMap` from
/// `UnresolvedFrames` to isize (count).
pub struct UnresolvedReport {
  /// key is a backtrace captured by profiler and value is count of it.
  pub data: HashMap<UnresolvedFrames, isize>,

  /// Collection frequency, start time, duration.
  pub timing: ReportTiming,
}

type FramesPostProcessor = Box<dyn Fn(&mut Frames)>;

fn add_positive_count<T>(hash_map: &mut HashMap<T, isize>, key: T, count: isize)
where
  T: Eq + Hash,
{
  if count <= 0 {
    return;
  }

  match hash_map.entry(key) {
    Entry::Occupied(mut existing_count) => {
      *existing_count.get_mut() += count;
    }
    Entry::Vacant(vacant_count) => {
      vacant_count.insert(count);
    }
  }
}

fn frames_from_positive_entry(
  entry: &crate::collector::Entry<UnresolvedFrames>,
  frames_post_processor: Option<&FramesPostProcessor>,
) -> Option<(Frames, isize)> {
  if entry.count <= 0 {
    return None;
  }

  let mut key = Frames::from(entry.item.clone());
  if let Some(processor) = frames_post_processor {
    processor(&mut key);
  }
  Some((key, entry.count))
}

/// A builder of `Report` and `UnresolvedReport`. It builds report from a running `Profiler`.
pub struct ReportBuilder<'a> {
  frames_post_processor: Option<FramesPostProcessor>,
  profiler:              &'a RwLock<Result<Profiler>>,
  timing:                ReportTiming,
}

impl<'a> ReportBuilder<'a> {
  pub(crate) fn new(profiler: &'a RwLock<Result<Profiler>>, timing: ReportTiming) -> Self {
    Self {
      frames_post_processor: None,
      profiler,
      timing,
    }
  }

  /// Set `frames_post_processor` of a `ReportBuilder`. Before finally building a report,
  /// `frames_post_processor` will be applied to every Frames.
  pub fn frames_post_processor<T>(&mut self, frames_post_processor: T) -> &mut Self
  where
    T: Fn(&mut Frames) + 'static,
  {
    self.frames_post_processor.replace(Box::new(frames_post_processor));

    self
  }

  /// Build an `UnresolvedReport`
  pub fn build_unresolved(&self) -> Result<UnresolvedReport> {
    let mut hash_map = HashMap::new();

    match self.profiler.read().as_ref() {
      Err(err) => {
        log::error!("error initializing profiler: {}", err);
        Err(Error::ProfilerInitialization)
      }
      Ok(profiler) => {
        for entry in profiler.data.try_iter()? {
          add_positive_count(&mut hash_map, entry.item.clone(), entry.count);
        }

        Ok(UnresolvedReport {
          data:   hash_map,
          timing: self.timing.clone(),
        })
      }
    }
  }

  /// Build a `Report`.
  pub fn build(&self) -> Result<Report> {
    let mut hash_map = HashMap::new();

    match self.profiler.write().as_mut() {
      Err(err) => {
        log::error!("error initializing profiler: {}", err);
        Err(Error::ProfilerInitialization)
      }
      Ok(profiler) => {
        for (key, count) in profiler
          .data
          .try_iter()?
          .filter_map(|entry| frames_from_positive_entry(entry, self.frames_post_processor.as_ref()))
        {
          add_positive_count(&mut hash_map, key, count);
        }

        Ok(Report {
          data:   hash_map,
          timing: self.timing.clone(),
        })
      }
    }
  }
}

/// This will generate Report in a human-readable format:
///
/// ```shell
/// FRAME: pprof::profiler::perf_signal_handler::h7b995c4ab2e66493 -> FRAME: Unknown -> FRAME: {func1} ->
/// FRAME: {func2} -> FRAME: {func3} ->  THREAD: {thread_name} {count}
/// ```
impl Debug for Report {
  fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
    for (key, rendered_count) in self.data.iter() {
      write!(f, "{:?} {}", key, rendered_count)?;
      writeln!(f)?;
    }

    Ok(())
  }
}

#[cfg(feature = "flamegraph")]
impl Report {
  /// `flamegraph` writes an SVG flamegraph into `writer`.
  pub fn flamegraph<W>(&self, writer: W) -> Result<()>
  where
    W: std::io::Write,
  {
    self.flamegraph_with_options(writer, &crate::flamegraph::Options::default())
  }

  /// `flamegraph_with_options` writes an SVG flamegraph with custom rendering options.
  pub fn flamegraph_with_options<W>(&self, writer: W, options: &crate::flamegraph::Options) -> Result<()>
  where
    W: std::io::Write,
  {
    crate::flamegraph::write_report(self, writer, options)
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::time::SystemTime;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;
  use strict_test_support::ensure_some;

  use super::ReportBuilder;
  use crate::Error;
  use crate::frames::UnresolvedFrames;
  use crate::profiler::Profiler;
  use crate::timer::ReportTiming;

  fn sample(thread_name: &[u8], thread_id: u64) -> UnresolvedFrames {
    UnresolvedFrames::new(Default::default(), thread_name, thread_id, SystemTime::UNIX_EPOCH)
  }

  fn profiler_with_samples(samples: Vec<(UnresolvedFrames, isize)>) -> std::result::Result<Profiler, TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    for (sample, count) in samples {
      ensure_ok(profiler.data.add(sample, count), "sample should be added to profiler data")?;
    }
    Ok(profiler)
  }

  #[test]
  fn report_builder_maps_profiler_initialization_errors() -> std::result::Result<(), TestFailure> {
    let profiler = spin::RwLock::new(Err(Error::ProfilerInitialization));
    let builder = ReportBuilder::new(&profiler, ReportTiming::default());

    ensure(
      builder.build_unresolved().err().map(|err| err.to_string()) == Some("profiler failed to initialize".to_owned()),
      "unresolved report should surface profiler initialization errors",
    )?;
    ensure(
      builder.build().err().map(|err| err.to_string()) == Some("profiler failed to initialize".to_owned()),
      "resolved report should surface profiler initialization errors",
    )
  }

  #[test]
  fn report_builder_merges_duplicate_unresolved_samples() -> std::result::Result<(), TestFailure> {
    let repeated_sample = sample(b"worker", 7);
    let profiler = spin::RwLock::new(Ok(profiler_with_samples(vec![(repeated_sample.clone(), 2), (repeated_sample, 3)])?));
    let unresolved = ensure_ok(
      ReportBuilder::new(&profiler, ReportTiming::default()).build_unresolved(),
      "unresolved report should build",
    )?;

    ensure_eq(&unresolved.data.len(), &1, "duplicate unresolved samples should merge")?;
    let (_sample, count) = ensure_some(unresolved.data.iter().next(), "merged unresolved report should contain one sample")?;
    ensure_eq(count, &5, "merged unresolved sample should preserve total count")
  }

  #[test]
  fn report_builder_ignores_non_positive_samples() -> std::result::Result<(), TestFailure> {
    let profiler = spin::RwLock::new(Ok(profiler_with_samples(vec![
      (sample(b"zero", 1), 0),
      (sample(b"negative", 2), -1),
    ])?));
    let unresolved = ensure_ok(
      ReportBuilder::new(&profiler, ReportTiming::default()).build_unresolved(),
      "unresolved report should build",
    )?;
    let resolved = ensure_ok(
      ReportBuilder::new(&profiler, ReportTiming::default()).build(),
      "resolved report should build",
    )?;

    ensure(unresolved.data.is_empty(), "unresolved report should ignore non-positive samples")?;
    ensure(resolved.data.is_empty(), "resolved report should ignore non-positive samples")
  }

  #[test]
  fn report_builder_post_processor_can_normalize_and_merge_resolved_frames() -> std::result::Result<(), TestFailure> {
    let profiler = spin::RwLock::new(Ok(profiler_with_samples(vec![
      (sample(b"worker-a", 1), 1),
      (sample(b"worker-b", 2), 2),
    ])?));
    let mut builder = ReportBuilder::new(&profiler, ReportTiming::default());
    builder.frames_post_processor(|frames| {
      frames.thread_name = "normalized-worker".to_owned();
      frames.thread_id = 0;
      frames.sample_timestamp = SystemTime::UNIX_EPOCH;
    });

    let resolved = ensure_ok(builder.build(), "resolved report should build")?;

    ensure_eq(&resolved.data.len(), &1, "post-processed resolved frame keys should merge")?;
    let (frames, count) = ensure_some(resolved.data.iter().next(), "merged report should contain one sample")?;
    ensure_eq(count, &3, "post-processed samples should preserve total count")?;
    ensure_eq(
      &frames.thread_name,
      &"normalized-worker".to_owned(),
      "post processor should normalize thread name",
    )
  }

  #[test]
  fn report_builder_builds_empty_reports_with_timing() -> std::result::Result<(), TestFailure> {
    let profiler = spin::RwLock::new(Ok(ensure_ok(Profiler::new(), "profiler should initialize")?));
    let timing = ReportTiming {
      frequency:  250,
      start_time: SystemTime::UNIX_EPOCH,
      duration:   std::time::Duration::from_millis(5),
    };
    let builder = ReportBuilder::new(&profiler, timing.clone());

    let unresolved = ensure_ok(builder.build_unresolved(), "empty unresolved report should build")?;
    let resolved = ensure_ok(builder.build(), "empty resolved report should build")?;

    ensure(unresolved.data.is_empty(), "unresolved empty profiler should have no samples")?;
    ensure(resolved.data.is_empty(), "resolved empty profiler should have no samples")?;
    ensure_eq(&resolved.timing.frequency, &timing.frequency, "resolved timing should be cloned")?;
    ensure(unresolved.timing.duration == timing.duration, "unresolved timing should be cloned")
  }

  #[test]
  fn report_debug_renders_sample_counts() -> std::result::Result<(), TestFailure> {
    let frames = crate::frames::Frames {
      frames:           Vec::new(),
      thread_name:      "debug-thread".to_owned(),
      thread_id:        9,
      sample_timestamp: SystemTime::UNIX_EPOCH,
    };
    let report = super::Report {
      data:   HashMap::from([(frames, 4)]),
      timing: ReportTiming::default(),
    };

    ensure(
      format!("{report:?}").contains("debug-thread 4"),
      "debug report should include frame identity and count",
    )
  }
}

#[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
mod protobuf {
  use std::collections::HashSet;
  use std::time::SystemTime;

  use super::*;
  use crate::protos;

  const SAMPLES: &str = "samples";
  const COUNT: &str = "count";
  const CPU: &str = "cpu";
  const NANOSECONDS: &str = "nanoseconds";
  const THREAD: &str = "thread";

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_function(id: u64, name: i64, system_name: i64, filename: i64) -> protos::Function {
    protos::Function {
      id,
      name,
      system_name,
      filename,
      start_line: 0,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_function(id: u64, name: i64, system_name: i64, filename: i64) -> protos::Function {
    let mut function = protos::Function::new();
    function.set_id(id);
    function.set_name(name);
    function.set_system_name(system_name);
    function.set_filename(filename);
    function
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_line(function_id: u64, line: i64) -> protos::Line {
    protos::Line {
      function_id,
      line,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_line(function_id: u64, line: i64) -> protos::Line {
    let mut line_message = protos::Line::new();
    line_message.set_function_id(function_id);
    line_message.set_line(line);
    line_message
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_location(id: u64, line: protos::Line) -> protos::Location {
    protos::Location {
      id,
      mapping_id: 0,
      address: 0,
      line: vec![line],
      is_folded: false,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_location(id: u64, line: protos::Line) -> protos::Location {
    let mut location = protos::Location::new();
    location.set_id(id);
    location.set_line(std::iter::once(line));
    location
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_label(key: i64, str: i64) -> protos::Label {
    protos::Label {
      key,
      str,
      num: 0,
      num_unit: 0,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_label(key: i64, str: i64) -> protos::Label {
    let mut label = protos::Label::new();
    label.set_key(key);
    label.set_str(str);
    label
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_sample(location_id: Vec<u64>, sample_values: Vec<i64>, label: protos::Label) -> protos::Sample {
    protos::Sample {
      location_id,
      value: sample_values,
      label: vec![label],
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_sample(location_id: Vec<u64>, sample_values: Vec<i64>, label: protos::Label) -> protos::Sample {
    let mut sample = protos::Sample::new();
    sample.set_location_id(location_id.into_iter());
    sample.set_value(sample_values.into_iter());
    sample.set_label(std::iter::once(label));
    sample
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_value_type(ty: i64, unit: i64) -> protos::ValueType {
    protos::ValueType {
      ty,
      unit,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_value_type(ty: i64, unit: i64) -> protos::ValueType {
    let mut value_type = protos::ValueType::new();
    value_type.set_ty(ty);
    value_type.set_unit(unit);
    value_type
  }

  struct ProfileParts {
    sample_type:    Vec<protos::ValueType>,
    samples:        Vec<protos::Sample>,
    string_table:   Vec<String>,
    functions:      Vec<protos::Function>,
    locations:      Vec<protos::Location>,
    period_type:    protos::ValueType,
    time_nanos:     i64,
    duration_nanos: i64,
    period:         i64,
  }

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  fn new_profile(parts: ProfileParts) -> protos::Profile {
    protos::Profile {
      sample_type:         parts.sample_type,
      sample:              parts.samples,
      mapping:             Vec::new(),
      location:            parts.locations,
      function:            parts.functions,
      string_table:        parts.string_table,
      drop_frames:         0,
      keep_frames:         0,
      time_nanos:          parts.time_nanos,
      duration_nanos:      parts.duration_nanos,
      period_type:         Some(parts.period_type),
      period:              parts.period,
      comment:             Vec::new(),
      default_sample_type: 0,
    }
  }

  #[cfg(feature = "protobuf-codec")]
  fn new_profile(parts: ProfileParts) -> protos::Profile {
    let mut profile = protos::Profile::new();
    profile.set_sample_type(parts.sample_type.into_iter());
    profile.set_sample(parts.samples.into_iter());
    profile.set_string_table(parts.string_table.into_iter());
    profile.set_function(parts.functions.into_iter());
    profile.set_location(parts.locations.into_iter());
    profile.set_time_nanos(parts.time_nanos);
    profile.set_duration_nanos(parts.duration_nanos);
    profile.set_period_type(parts.period_type);
    profile.set_period(parts.period);
    profile
  }

  fn insert_frame_strings(dedup_str: &mut HashSet<String>, key: &Frames) {
    dedup_str.insert(key.thread_name_or_id());
    for symbol in key.frames.iter().flatten() {
      dedup_str.insert(symbol.name());
      dedup_str.insert(symbol.sys_name().into_owned());
      dedup_str.insert(symbol.filename().into_owned());
    }
  }

  fn location_ids_for_frames(
    key: &Frames,
    functions: &mut HashMap<String, u64>,
    strings: &HashMap<&str, usize>,
    function_table: &mut Vec<protos::Function>,
    location_table: &mut Vec<protos::Location>,
  ) -> Vec<u64> {
    key
      .frames
      .iter()
      .flatten()
      .map(|symbol| location_id_for_symbol(symbol, functions, strings, function_table, location_table))
      .collect()
  }

  fn location_id_for_symbol(
    symbol: &crate::frames::Symbol,
    functions: &mut HashMap<String, u64>,
    strings: &HashMap<&str, usize>,
    function_table: &mut Vec<protos::Function>,
    location_table: &mut Vec<protos::Location>,
  ) -> u64 {
    let name = symbol.name();
    if let Some(location_id) = functions.get(&name) {
      return *location_id;
    }

    let sys_name = symbol.sys_name();
    let filename = symbol.filename();
    let lineno = symbol.lineno();
    let function_id = function_table.len() as u64 + 1;
    let function = new_function(
      function_id,
      *strings.get(name.as_str()).unwrap() as i64,
      *strings.get(sys_name.as_ref()).unwrap() as i64,
      *strings.get(filename.as_ref()).unwrap() as i64,
    );
    functions.insert(name, function_id);
    let line = new_line(function_id, lineno as i64);
    let location = new_location(function_id, line);
    function_table.push(function);
    location_table.push(location);
    function_id
  }

  impl Report {
    /// `pprof` will generate google's pprof format report.
    pub fn pprof(&self) -> crate::Result<protos::Profile> {
      if self.timing.frequency <= 0 {
        return Err(Error::InvalidFrequency(self.timing.frequency));
      }

      let mut dedup_str = HashSet::new();
      for key in self.data.keys() {
        insert_frame_strings(&mut dedup_str, key);
      }
      dedup_str.insert(SAMPLES.into());
      dedup_str.insert(COUNT.into());
      dedup_str.insert(CPU.into());
      dedup_str.insert(NANOSECONDS.into());
      dedup_str.insert(THREAD.into());
      // string table's first element must be an empty string
      let mut str_tbl = vec!["".to_owned()];
      str_tbl.extend(dedup_str);

      let mut strings = HashMap::new();
      for (index, name) in str_tbl.iter().enumerate() {
        strings.insert(name.as_str(), index);
      }

      let mut samples = vec![];
      let mut loc_tbl = vec![];
      let mut fn_tbl = vec![];
      let mut functions = HashMap::new();
      for (key, count) in self.data.iter() {
        let locs = location_ids_for_frames(key, &mut functions, &strings, &mut fn_tbl, &mut loc_tbl);
        let thread_name = new_label(
          *strings.get(THREAD).unwrap() as i64,
          *strings.get(&key.thread_name_or_id().as_str()).unwrap() as i64,
        );
        let sample = new_sample(
          locs,
          vec![*count as i64, *count as i64 * 1_000_000_000 / self.timing.frequency as i64],
          thread_name,
        );
        samples.push(sample);
      }
      let samples_value = new_value_type(*strings.get(SAMPLES).unwrap() as i64, *strings.get(COUNT).unwrap() as i64);
      let sample_time_value = new_value_type(*strings.get(CPU).unwrap() as i64, *strings.get(NANOSECONDS).unwrap() as i64);
      let period_time_value = new_value_type(*strings.get(CPU).unwrap() as i64, *strings.get(NANOSECONDS).unwrap() as i64);
      let time_nanos = self
        .timing
        .start_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64;
      let duration_nanos = self.timing.duration.as_nanos() as i64;
      let period = 1_000_000_000 / self.timing.frequency as i64;
      let profile = new_profile(ProfileParts {
        sample_type: vec![samples_value, sample_time_value],
        samples,
        string_table: str_tbl,
        functions: fn_tbl,
        locations: loc_tbl,
        period_type: period_time_value,
        time_nanos,
        duration_nanos,
        period,
      });
      Ok(profile)
    }
  }

  #[cfg(test)]
  mod tests {
    use std::time::Duration;

    use strict_test_support::TestFailure;
    use strict_test_support::ensure;
    use strict_test_support::ensure_eq;
    use strict_test_support::ensure_ok;
    use strict_test_support::ensure_some;

    use super::*;
    use crate::frames::Symbol;

    struct ValueTypeData {
      ty:   i64,
      unit: i64,
    }

    struct LabelData {
      key:       i64,
      str_index: i64,
    }

    struct SampleData {
      location_id: Vec<u64>,
      value:       Vec<i64>,
      label:       Vec<LabelData>,
    }

    struct LineData {
      line: i64,
    }

    struct LocationData {
      line: Vec<LineData>,
    }

    struct FunctionData {
      name:     i64,
      filename: i64,
    }

    struct ProfileData {
      sample_type:    Vec<ValueTypeData>,
      sample:         Vec<SampleData>,
      string_table:   Vec<String>,
      function:       Vec<FunctionData>,
      location:       Vec<LocationData>,
      period:         i64,
      duration_nanos: i64,
      time_nanos:     i64,
    }

    #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
    fn profile_data(profile: &protos::Profile) -> std::result::Result<ProfileData, TestFailure> {
      Ok(ProfileData {
        sample_type:    profile
          .sample_type
          .iter()
          .map(|value_type| ValueTypeData {
            ty:   value_type.ty,
            unit: value_type.unit,
          })
          .collect(),
        sample:         profile
          .sample
          .iter()
          .map(|sample| SampleData {
            location_id: sample.location_id.clone(),
            value:       sample.value.clone(),
            label:       sample
              .label
              .iter()
              .map(|label| LabelData {
                key:       label.key,
                str_index: label.str,
              })
              .collect(),
          })
          .collect(),
        string_table:   profile.string_table.clone(),
        function:       profile
          .function
          .iter()
          .map(|function| FunctionData {
            name:     function.name,
            filename: function.filename,
          })
          .collect(),
        location:       profile
          .location
          .iter()
          .map(|location| LocationData {
            line: location
              .line
              .iter()
              .map(|line| LineData {
                line: line.line
              })
              .collect(),
          })
          .collect(),
        period:         profile.period,
        duration_nanos: profile.duration_nanos,
        time_nanos:     profile.time_nanos,
      })
    }

    #[cfg(feature = "protobuf-codec")]
    fn profile_data(profile: &protos::Profile) -> std::result::Result<ProfileData, TestFailure> {
      Ok(ProfileData {
        sample_type:    profile
          .sample_type()
          .iter()
          .map(|value_type| ValueTypeData {
            ty:   value_type.ty(),
            unit: value_type.unit(),
          })
          .collect(),
        sample:         profile
          .sample()
          .iter()
          .map(|sample| SampleData {
            location_id: sample.location_id().iter().collect(),
            value:       sample.value().iter().collect(),
            label:       sample
              .label()
              .iter()
              .map(|label| LabelData {
                key:       label.key(),
                str_index: label.str(),
              })
              .collect(),
          })
          .collect(),
        string_table:   profile
          .string_table()
          .iter()
          .map(|value| {
            ensure_ok(
              value.to_str().map(std::borrow::ToOwned::to_owned),
              "string table entry should be valid utf-8",
            )
          })
          .collect::<std::result::Result<Vec<_>, _>>()?,
        function:       profile
          .function()
          .iter()
          .map(|function| FunctionData {
            name:     function.name(),
            filename: function.filename(),
          })
          .collect(),
        location:       profile
          .location()
          .iter()
          .map(|location| LocationData {
            line: location
              .line()
              .iter()
              .map(|line| LineData {
                line: line.line()
              })
              .collect(),
          })
          .collect(),
        period:         profile.period(),
        duration_nanos: profile.duration_nanos(),
        time_nanos:     profile.time_nanos(),
      })
    }

    fn string_at(profile: &ProfileData, index: i64) -> std::result::Result<&str, TestFailure> {
      let index = ensure_ok(usize::try_from(index), "string table index should fit usize")?;
      let value = ensure_some(
        profile.string_table.get(index),
        "string table index should reference an existing entry",
      )?;
      Ok(value.as_str())
    }

    fn report(report_data: HashMap<Frames, isize>, frequency: i32) -> Report {
      Report {
        data:   report_data,
        timing: ReportTiming {
          frequency,
          start_time: SystemTime::UNIX_EPOCH,
          duration: Duration::from_millis(250),
        },
      }
    }

    fn test_symbol(name: &[u8], lineno: u32, filename: &str) -> Symbol {
      Symbol {
        name:     Some(name.to_vec()),
        addr:     None,
        lineno:   Some(lineno),
        filename: Some(filename.into()),
      }
    }

    fn test_frames(thread_name: &str, thread_id: u64, stack: Vec<Vec<Symbol>>) -> Frames {
      Frames {
        frames: stack,
        thread_name: thread_name.to_owned(),
        thread_id,
        sample_timestamp: SystemTime::UNIX_EPOCH,
      }
    }

    #[test]
    fn pprof_empty_report_has_required_metadata() -> std::result::Result<(), TestFailure> {
      let profile = profile_data(&ensure_ok(report(HashMap::new(), 100).pprof(), "empty pprof report should build")?)?;

      ensure(profile.sample.is_empty(), "empty report should not contain samples")?;
      ensure(string_at(&profile, 0)?.is_empty(), "first string table entry should be empty")?;
      ensure_eq(&profile.sample_type.len(), &2, "profile should contain sample and cpu value types")?;
      ensure(
        string_at(&profile, profile.sample_type[0].ty)? == SAMPLES,
        "first value type should be samples",
      )?;
      ensure(
        string_at(&profile, profile.sample_type[0].unit)? == COUNT,
        "first value unit should be count",
      )?;
      ensure(
        string_at(&profile, profile.sample_type[1].ty)? == CPU,
        "second value type should be cpu",
      )?;
      ensure(
        string_at(&profile, profile.sample_type[1].unit)? == NANOSECONDS,
        "second value unit should be nanoseconds",
      )?;
      ensure_eq(&profile.period, &10_000_000, "profile period should match frequency")?;
      ensure_eq(&profile.duration_nanos, &250_000_000, "profile duration should be encoded")
    }

    #[test]
    fn pprof_rejects_non_positive_frequency() -> std::result::Result<(), TestFailure> {
      for frequency in [0, -1] {
        ensure(
          report(HashMap::new(), frequency).pprof().err().map(|err| err.to_string())
            == Some(format!(
              "invalid profiler frequency {frequency}; expected a positive sampling frequency"
            )),
          "pprof report should reject non-positive frequencies before computing period or sample duration",
        )?;
      }
      Ok(())
    }

    #[test]
    fn pprof_report_maps_thread_label_and_sample_values() -> std::result::Result<(), TestFailure> {
      let frames = test_frames("worker-thread", 7, vec![vec![test_symbol(b"sampled_function", 42, "src/lib.rs")]]);
      let report_data = HashMap::from([(frames, 3)]);

      let profile = profile_data(&ensure_ok(report(report_data, 100).pprof(), "pprof report should build")?)?;

      ensure_eq(&profile.sample.len(), &1, "profile should contain one sample")?;
      let sample = &profile.sample[0];
      ensure(
        sample.value == vec![3, 30_000_000],
        "sample values should contain count and cpu nanos",
      )?;
      ensure(sample.location_id == vec![1], "sample should point at first location")?;
      ensure_eq(&profile.function.len(), &1, "profile should contain one function")?;
      ensure(
        string_at(&profile, profile.function[0].name)? == "sampled_function",
        "function name should use string table",
      )?;
      ensure(
        string_at(&profile, profile.function[0].filename)? == "src/lib.rs",
        "filename should use string table",
      )?;
      ensure_eq(&profile.location.len(), &1, "profile should contain one location")?;
      ensure_eq(&profile.location[0].line[0].line, &42, "line number should be encoded")?;

      let thread_label = ensure_some(
        sample
          .label
          .iter()
          .find(|label| string_at(&profile, label.key).is_ok_and(|value| value == THREAD)),
        "thread label should be present",
      )?;
      ensure(
        string_at(&profile, thread_label.str_index)? == "worker-thread",
        "thread label should use thread name",
      )
    }

    #[test]
    fn pprof_report_uses_thread_id_label_when_thread_name_is_empty() -> std::result::Result<(), TestFailure> {
      let frames = test_frames("", 42, vec![vec![test_symbol(b"sampled_function", 42, "src/lib.rs")]]);
      let report_data = HashMap::from([(frames, 1)]);

      let profile = profile_data(&ensure_ok(report(report_data, 100).pprof(), "pprof report should build")?)?;
      let sample = ensure_some(profile.sample.first(), "profile should contain one sample")?;
      let thread_label = ensure_some(
        sample
          .label
          .iter()
          .find(|label| string_at(&profile, label.key).is_ok_and(|value| value == THREAD)),
        "thread label should be present",
      )?;

      ensure(
        string_at(&profile, thread_label.str_index)? == "42",
        "unnamed thread labels should fall back to the thread id",
      )
    }

    #[test]
    fn pprof_preserves_location_order_for_multi_frame_samples() -> std::result::Result<(), TestFailure> {
      let frames = test_frames("ordered-thread", 7, vec![
        vec![test_symbol(b"outer_function", 10, "src/outer.rs")],
        vec![test_symbol(b"inner_function", 20, "src/inner.rs")],
      ]);

      let profile = profile_data(&ensure_ok(
        report(HashMap::from([(frames, 1)]), 100).pprof(),
        "multi-frame pprof report should build",
      )?)?;
      let sample = ensure_some(profile.sample.first(), "profile should contain one sample")?;

      ensure(
        sample.location_id == vec![1, 2],
        "pprof sample should preserve flattened frame location order",
      )?;
      ensure_eq(&profile.function.len(), &2, "profile should contain both functions")?;
      ensure(
        string_at(&profile, profile.function[0].name)? == "outer_function",
        "first function should correspond to the first frame",
      )?;
      ensure(
        string_at(&profile, profile.function[1].name)? == "inner_function",
        "second function should correspond to the second frame",
      )?;
      ensure_eq(
        &profile.location[0].line[0].line,
        &10,
        "first location should preserve first frame line",
      )?;
      ensure_eq(
        &profile.location[1].line[0].line,
        &20,
        "second location should preserve second frame line",
      )
    }

    #[test]
    fn pprof_thread_label_uses_thread_id_when_name_is_empty() -> std::result::Result<(), TestFailure> {
      let frames = test_frames("", 9, Vec::new());
      let profile = profile_data(&ensure_ok(
        report(HashMap::from([(frames, 1)]), 100).pprof(),
        "pprof report should build",
      )?)?;
      let sample = ensure_some(profile.sample.first(), "profile should contain sample")?;
      let thread_label = ensure_some(
        sample
          .label
          .iter()
          .find(|label| string_at(&profile, label.key).is_ok_and(|value| value == THREAD)),
        "thread label should be present",
      )?;

      ensure(
        string_at(&profile, thread_label.str_index)? == "9",
        "thread label should use thread id when thread name is empty",
      )
    }

    #[test]
    fn pprof_reuses_locations_for_duplicate_symbol_names() -> std::result::Result<(), TestFailure> {
      let first = test_frames("first-thread", 1, vec![vec![test_symbol(b"shared_function", 7, "src/shared.rs")]]);
      let second = test_frames("second-thread", 2, vec![vec![test_symbol(b"shared_function", 7, "src/shared.rs")]]);
      let profile = profile_data(&ensure_ok(
        report(HashMap::from([(first, 1), (second, 2)]), 100).pprof(),
        "pprof report should build",
      )?)?;

      ensure_eq(&profile.sample.len(), &2, "profile should contain both samples")?;
      ensure_eq(&profile.function.len(), &1, "duplicate symbol names should share one function")?;
      ensure_eq(&profile.location.len(), &1, "duplicate symbol names should share one location")?;
      ensure(
        profile.sample.iter().all(|sample| sample.location_id == vec![1]),
        "both samples should point at reused location",
      )
    }

    #[test]
    fn pprof_clamps_pre_epoch_start_time_to_zero() -> std::result::Result<(), TestFailure> {
      let report = Report {
        data:   HashMap::new(),
        timing: ReportTiming {
          frequency:  100,
          start_time: SystemTime::UNIX_EPOCH - Duration::from_secs(1),
          duration:   Duration::from_millis(1),
        },
      };

      let profile = profile_data(&ensure_ok(report.pprof(), "pre-epoch pprof report should build")?)?;

      ensure_eq(
        &profile.time_nanos,
        &0,
        "pprof report should clamp pre-epoch start times to zero nanos",
      )
    }
  }
}
