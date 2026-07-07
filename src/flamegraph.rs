use std::collections::BTreeMap;
use std::io::Write;

use svg::Document;
use svg::node::element::Rectangle;
use svg::node::element::Text;

use crate::Error;
use crate::Report;
use crate::Result;

const DEFAULT_IMAGE_WIDTH: u32 = 1200;
const DEFAULT_FRAME_HEIGHT: u32 = 16;
const DEFAULT_FONT_SIZE: u32 = 12;
const DEFAULT_FONT_FAMILY: &str = "Verdana";
const DEFAULT_TITLE: &str = "Flamegraph";
const DEFAULT_MIN_FRAME_WIDTH: u32 = 1;
const TITLE_BASELINE: u32 = 20;
const TITLE_HEIGHT: u32 = 28;
const TEXT_PADDING_X: f64 = 3.0;

/// Rendering options for `pprof` SVG flamegraphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
  /// Explicit SVG image width. `None` uses the default width.
  pub image_width:     Option<u32>,
  /// Height in pixels for each stack frame rectangle.
  pub frame_height:    u32,
  /// Font size in pixels for title and frame labels.
  pub font_size:       u32,
  /// Font family used for SVG text.
  pub font_family:     String,
  /// Flamegraph title.
  pub title:           String,
  /// Minimum frame width in pixels before a frame is rendered.
  pub min_frame_width: u32,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      image_width:     Some(DEFAULT_IMAGE_WIDTH),
      frame_height:    DEFAULT_FRAME_HEIGHT,
      font_size:       DEFAULT_FONT_SIZE,
      font_family:     DEFAULT_FONT_FAMILY.to_owned(),
      title:           DEFAULT_TITLE.to_owned(),
      min_frame_width: DEFAULT_MIN_FRAME_WIDTH,
    }
  }
}

struct ValidatedOptions<'a> {
  image_width:     u32,
  frame_height:    u32,
  font_size:       u32,
  font_family:     &'a str,
  title:           &'a str,
  min_frame_width: u32,
}

#[derive(Default)]
struct FlameNode {
  count:    u64,
  children: BTreeMap<String, FlameNode>,
}

impl FlameNode {
  fn insert_stack(&mut self, stack: &[String], count: u64) {
    self.count = self.count.saturating_add(count);

    let Some((label, remaining_stack)) = stack.split_first() else {
      return;
    };

    self
      .children
      .entry(label.clone())
      .or_default()
      .insert_stack(remaining_stack, count);
  }

  fn max_depth(&self) -> usize {
    self
      .children
      .values()
      .map(|child| child.max_depth().saturating_add(1))
      .max()
      .unwrap_or_default()
  }
}

pub(crate) fn write_report<W>(report: &Report, writer: W, options: &Options) -> Result<()>
where
  W: Write,
{
  let options = validate_options(options)?;
  let root = aggregate_report(report);
  let document = render_document(&root, &options);
  svg::write(writer, &document).map_err(Error::FlamegraphOutput)
}

fn validate_options(options: &Options) -> Result<ValidatedOptions<'_>> {
  let image_width = match options.image_width {
    Some(0) => return Err(Error::InvalidFlamegraphOptions("image_width must be greater than zero".to_owned())),
    Some(width) => width,
    None => DEFAULT_IMAGE_WIDTH,
  };

  if options.frame_height == 0 {
    return Err(Error::InvalidFlamegraphOptions("frame_height must be greater than zero".to_owned()));
  }
  if options.font_size == 0 {
    return Err(Error::InvalidFlamegraphOptions("font_size must be greater than zero".to_owned()));
  }
  if options.min_frame_width == 0 {
    return Err(Error::InvalidFlamegraphOptions(
      "min_frame_width must be greater than zero".to_owned(),
    ));
  }

  Ok(ValidatedOptions {
    image_width,
    frame_height: options.frame_height,
    font_size: options.font_size,
    font_family: &options.font_family,
    title: &options.title,
    min_frame_width: options.min_frame_width,
  })
}

fn aggregate_report(report: &Report) -> FlameNode {
  let mut root = FlameNode::default();

  for (frames, count) in &report.data {
    let Ok(sample_count) = u64::try_from(*count) else {
      continue;
    };
    if sample_count == 0 {
      continue;
    }

    let stack = stack_path(frames);
    root.insert_stack(&stack, sample_count);
  }

  root
}

fn stack_path(frames: &crate::Frames) -> Vec<String> {
  let mut path = vec![frames.thread_name_or_id()];
  for frame in frames.frames.iter().rev() {
    for symbol in frame.iter().rev() {
      path.push(symbol.name());
    }
  }
  path
}

fn render_document(root: &FlameNode, options: &ValidatedOptions<'_>) -> Document {
  let stack_height = u32::try_from(root.max_depth())
    .unwrap_or(u32::MAX)
    .saturating_mul(options.frame_height);
  let image_height = TITLE_HEIGHT.saturating_add(stack_height).saturating_add(4);

  let document = Document::new()
    .set("xmlns", "http://www.w3.org/2000/svg")
    .set("version", "1.1")
    .set("width", options.image_width)
    .set("height", image_height)
    .set("viewBox", (0, 0, options.image_width, image_height))
    .add(
      Text::new(options.title)
        .set("x", 8)
        .set("y", TITLE_BASELINE)
        .set("font-size", options.font_size)
        .set("font-family", options.font_family)
        .set("fill", "#111111"),
    );

  if root.count == 0 {
    return document;
  }

  render_children(document, root, 0, 0.0, f64::from(options.image_width), options)
}

fn render_children(
  mut document: Document,
  parent: &FlameNode,
  depth: usize,
  x: f64,
  width: f64,
  options: &ValidatedOptions<'_>,
) -> Document {
  let mut child_x = x;
  let parent_count = parent.count;
  for (label, child) in &parent.children {
    let child_width = width * child.count as f64 / parent_count as f64;
    if child_width < f64::from(options.min_frame_width) {
      child_x += child_width;
      continue;
    }

    let y = f64::from(TITLE_HEIGHT) + u32::try_from(depth).map_or(f64::from(u32::MAX), f64::from) * f64::from(options.frame_height);
    document = document.add(frame_rectangle(label, child_x, y, child_width, options));
    if label_fits(label, child_width, options.font_size) {
      document = document.add(frame_label(label, child_x, y, options));
    }
    document = render_children(document, child, depth.saturating_add(1), child_x, child_width, options);
    child_x += child_width;
  }
  document
}

fn frame_rectangle(label: &str, x: f64, y: f64, width: f64, options: &ValidatedOptions<'_>) -> Rectangle {
  Rectangle::new()
    .set("class", "sample-frame")
    .set("x", x)
    .set("y", y)
    .set("width", width)
    .set("height", options.frame_height)
    .set("fill", color_for_label(label))
    .set("stroke", "#ffffff")
    .set("stroke-width", 0.5)
}

fn frame_label(label: &str, x: f64, y: f64, options: &ValidatedOptions<'_>) -> Text {
  Text::new(label)
    .set("x", x + TEXT_PADDING_X)
    .set("y", y + f64::from(options.frame_height.saturating_sub(4)))
    .set("font-size", options.font_size)
    .set("font-family", options.font_family)
    .set("fill", "#111111")
}

fn label_fits(label: &str, width: f64, font_size: u32) -> bool {
  let character_count = u32::try_from(label.chars().count()).unwrap_or(u32::MAX);
  let estimated_width = f64::from(character_count) * f64::from(font_size) * 0.6 + TEXT_PADDING_X * 2.0;
  width >= estimated_width
}

fn color_for_label(label: &str) -> String {
  let mut hash = 0xcbf29ce484222325_u64;
  for byte in label.bytes() {
    hash ^= u64::from(byte);
    hash = hash.wrapping_mul(0x100000001b3);
  }

  let red = 120_u8.saturating_add(u8::try_from(hash & 0x5f).unwrap_or_default());
  let green = 90_u8.saturating_add(u8::try_from((hash >> 8) & 0x5f).unwrap_or_default());
  let blue = 70_u8.saturating_add(u8::try_from((hash >> 16) & 0x5f).unwrap_or_default());

  format!("#{red:02x}{green:02x}{blue:02x}")
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::io;
  use std::time::Duration;
  use std::time::SystemTime;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_contains;
  use strict_test_support::ensure_lacks;
  use strict_test_support::ensure_ok;

  use super::*;
  use crate::Frames;
  use crate::frames::Symbol;
  use crate::timer::ReportTiming;

  fn symbol(name: &str) -> Symbol {
    Symbol {
      name:     Some(name.as_bytes().to_vec()),
      addr:     None,
      lineno:   None,
      filename: None,
    }
  }

  fn frames(thread_name: &str, stack_path: &[&str]) -> Frames {
    Frames {
      frames:           stack_path.iter().rev().map(|name| vec![symbol(name)]).collect(),
      thread_name:      thread_name.to_owned(),
      thread_id:        7,
      sample_timestamp: SystemTime::UNIX_EPOCH,
    }
  }

  fn report(entries: Vec<(Frames, isize)>) -> Report {
    Report {
      data:   HashMap::from_iter(entries),
      timing: ReportTiming {
        frequency:  100,
        start_time: SystemTime::UNIX_EPOCH,
        duration:   Duration::from_millis(250),
      },
    }
  }

  fn render(report: &Report) -> Result<String> {
    let mut output = Vec::new();
    report.flamegraph(&mut output)?;
    String::from_utf8(output).map_err(|err| Error::InvalidFlamegraphOptions(err.to_string()))
  }

  fn render_with_options(report: &Report, options: &Options) -> Result<String> {
    let mut output = Vec::new();
    report.flamegraph_with_options(&mut output, options)?;
    String::from_utf8(output).map_err(|err| Error::InvalidFlamegraphOptions(err.to_string()))
  }

  #[test]
  fn default_options_match_public_contract() -> std::result::Result<(), TestFailure> {
    let options = Options::default();

    ensure(options.image_width == Some(1200), "default image width should be 1200")?;
    ensure(options.frame_height == 16, "default frame height should be 16")?;
    ensure(options.font_size == 12, "default font size should be 12")?;
    ensure(options.font_family == "Verdana", "default font family should be Verdana")?;
    ensure(options.title == "Flamegraph", "default title should be Flamegraph")?;
    ensure(options.min_frame_width == 1, "default minimum frame width should be one")
  }

  #[test]
  fn renders_report_as_svg() -> std::result::Result<(), TestFailure> {
    let report = report(vec![(frames("worker-thread", &["parent", "leaf"]), 3)]);

    let svg = ensure_ok(render(&report), "flamegraph should render")?;

    ensure_contains(&svg, "<svg", "svg root should be present")?;
    ensure_contains(&svg, DEFAULT_TITLE, "default title should be present")?;
    ensure_contains(&svg, "worker-thread", "thread label should be present")?;
    ensure_contains(&svg, "parent", "parent symbol should be present")?;
    ensure_contains(&svg, "leaf", "leaf symbol should be present")?;
    ensure_contains(&svg, "<rect", "sample rectangles should be present")
  }

  #[test]
  fn default_width_is_used_when_image_width_is_none() -> std::result::Result<(), TestFailure> {
    let report = report(vec![(frames("worker-thread", &["parent"]), 1)]);
    let options = Options {
      image_width: None,
      ..Options::default()
    };

    let svg = ensure_ok(render_with_options(&report, &options), "flamegraph should render")?;

    ensure_contains(&svg, "width=\"1200\"", "none image width should use default width")
  }

  #[test]
  fn custom_title_and_font_options_are_rendered() -> std::result::Result<(), TestFailure> {
    let report = report(vec![(frames("worker-thread", &["parent"]), 1)]);
    let options = Options {
      font_size: 14,
      font_family: "monospace".to_owned(),
      title: "Custom profile".to_owned(),
      ..Options::default()
    };

    let svg = ensure_ok(render_with_options(&report, &options), "flamegraph should render")?;

    ensure_contains(&svg, "Custom profile", "custom title should be rendered")?;
    ensure_contains(&svg, "font-family=\"monospace\"", "custom font family should be rendered")?;
    ensure_contains(&svg, "font-size=\"14\"", "custom font size should be rendered")
  }

  #[test]
  fn renders_identical_reports_deterministically() -> std::result::Result<(), TestFailure> {
    let report = report(vec![
      (frames("worker-b", &["parent", "child-b"]), 2),
      (frames("worker-a", &["parent", "child-a"]), 1),
    ]);

    let first_svg = ensure_ok(render(&report), "first flamegraph should render")?;
    let second_svg = ensure_ok(render(&report), "second flamegraph should render")?;

    ensure(first_svg == second_svg, "same report should render the same svg bytes")
  }

  #[test]
  fn aggregates_stacks_with_shared_parent() -> std::result::Result<(), TestFailure> {
    let report = report(vec![
      (frames("worker-thread", &["parent", "child-a"]), 2),
      (frames("worker-thread", &["parent", "child-b"]), 1),
    ]);

    let svg = ensure_ok(render(&report), "flamegraph should render")?;

    ensure_contains(&svg, "parent", "shared parent should be rendered")?;
    ensure_contains(&svg, "child-a", "first child should be rendered")?;
    ensure_contains(&svg, "child-b", "second child should be rendered")?;
    ensure(svg.matches("parent").count() == 1, "shared parent label should render once")
  }

  #[test]
  fn renders_sample_counts_as_proportional_widths() -> std::result::Result<(), TestFailure> {
    let report = report(vec![
      (frames("worker-thread", &["parent", "child-a"]), 2),
      (frames("worker-thread", &["parent", "child-b"]), 1),
    ]);
    let options = Options {
      image_width: Some(300),
      ..Options::default()
    };

    let svg = ensure_ok(render_with_options(&report, &options), "flamegraph should render")?;

    ensure_contains(&svg, "width=\"300\"", "thread and shared parent should span all samples")?;
    ensure_contains(&svg, "width=\"200\"", "two-count child should receive two thirds of width")?;
    ensure_contains(&svg, "width=\"100\"", "one-count child should receive one third of width")
  }

  #[test]
  fn ignores_non_positive_samples() -> std::result::Result<(), TestFailure> {
    let report = report(vec![
      (frames("positive-thread", &["positive-symbol"]), 1),
      (frames("zero-thread", &["zero-symbol"]), 0),
      (frames("negative-thread", &["negative-symbol"]), -1),
    ]);

    let svg = ensure_ok(render(&report), "flamegraph should render")?;

    ensure_contains(&svg, "positive-thread", "positive sample should render")?;
    ensure_contains(&svg, "positive-symbol", "positive stack should render")?;
    ensure_lacks(&svg, "zero-symbol", "zero-count sample should not render")?;
    ensure_lacks(&svg, "negative-symbol", "negative-count sample should not render")
  }

  #[test]
  fn culls_frames_below_minimum_width() -> std::result::Result<(), TestFailure> {
    let report = report(vec![(frames("worker-a", &["child-a"]), 1), (frames("worker-b", &["child-b"]), 1)]);
    let options = Options {
      image_width: Some(10),
      min_frame_width: 6,
      ..Options::default()
    };

    let svg = ensure_ok(render_with_options(&report, &options), "flamegraph should render")?;

    ensure_lacks(&svg, "sample-frame", "frames narrower than min width should be culled")
  }

  #[test]
  fn omits_labels_that_do_not_fit_narrow_frames() -> std::result::Result<(), TestFailure> {
    let long_symbol = "very_long_symbol_name_that_cannot_fit";
    let report = report(vec![(frames("worker-thread", &[long_symbol]), 1)]);
    let options = Options {
      image_width: Some(20),
      min_frame_width: 1,
      ..Options::default()
    };

    let svg = ensure_ok(render_with_options(&report, &options), "flamegraph should render")?;

    ensure_contains(&svg, "sample-frame", "narrow frames should still render rectangles")?;
    ensure_lacks(&svg, long_symbol, "label text should be omitted when it does not fit")
  }

  #[test]
  fn renders_empty_report_as_minimal_svg() -> std::result::Result<(), TestFailure> {
    let svg = ensure_ok(render(&report(Vec::new())), "empty flamegraph should render")?;

    ensure_contains(&svg, "<svg", "svg root should be present")?;
    ensure_contains(&svg, DEFAULT_TITLE, "default title should be present")?;
    ensure_lacks(&svg, "sample-frame", "empty report should not contain sample rectangles")
  }

  #[test]
  fn rejects_zero_image_width() -> std::result::Result<(), TestFailure> {
    let options = Options {
      image_width: Some(0),
      ..Options::default()
    };

    ensure(
      write_report(&report(Vec::new()), Vec::new(), &options)
        .err()
        .map(|err| err.to_string())
        == Some("invalid flamegraph options: image_width must be greater than zero".to_owned()),
      "zero image width should be rejected",
    )
  }

  #[test]
  fn rejects_zero_dimensions() -> std::result::Result<(), TestFailure> {
    for (options, expected_error) in [
      (
        Options {
          frame_height: 0,
          ..Options::default()
        },
        "invalid flamegraph options: frame_height must be greater than zero",
      ),
      (
        Options {
          font_size: 0,
          ..Options::default()
        },
        "invalid flamegraph options: font_size must be greater than zero",
      ),
      (
        Options {
          min_frame_width: 0,
          ..Options::default()
        },
        "invalid flamegraph options: min_frame_width must be greater than zero",
      ),
    ] {
      ensure(
        write_report(&report(Vec::new()), Vec::new(), &options)
          .err()
          .map(|err| err.to_string())
          == Some(expected_error.to_owned()),
        "zero flamegraph dimension should be rejected",
      )?;
    }
    Ok(())
  }

  #[test]
  fn maps_writer_failures() -> std::result::Result<(), TestFailure> {
    let report = report(vec![(frames("worker-thread", &["parent"]), 1)]);

    let error = report.flamegraph(FailingWriter).err();

    ensure_contains(
      &error.as_ref().map(ToString::to_string).unwrap_or_default(),
      "failed to write flamegraph SVG",
      "writer failure should map to flamegraph output error",
    )?;
    ensure(
      error.as_ref().and_then(std::error::Error::source).is_some(),
      "writer failure should preserve source error",
    )
  }

  struct FailingWriter;

  impl io::Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
      Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }
}
