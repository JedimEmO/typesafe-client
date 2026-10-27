//! Images in System One requests, enabled by the `images` feature.
//!
//! TypeSafe marks images and [`BoxQuestion`](crate::BoxQuestion)s as experimental
//! ("Dreamer-ready"): the request format, answers and limits can change between API
//! releases. This crate follows those changes, so the types in this module may change in
//! minor releases.
//!
//! An image is listed once in the request's [`Assets`], under a reference string. Write the
//! reference wherever the image belongs (in the state, in a question's instructions or in
//! its criteria) and the model receives the image in its place:
//!
//! ```
//! use serde_json::json;
//! use typesafe_client::{Assets, Image, NoulQuestion, Questions, SystemOneRequest};
//!
//! # let photo_bytes = include_bytes!("../tests/fixtures/red-rectangle.png").to_vec();
//! let mut assets = Assets::new();
//! let photo = assets.add(Image::png(photo_bytes));
//!
//! let mut questions = Questions::new();
//! let red = questions.add("red", NoulQuestion::new("Is there a red shape in `photo`?"));
//!
//! let request = SystemOneRequest::new(json!({"photo": photo}), questions).with_assets(assets);
//! request.validate()?;
//! # let _ = red;
//! # Ok::<(), typesafe_client::ValidationError>(())
//! ```
//!
//! The HTTP `Client` (feature `http`) uploads images as multipart file parts and sends the
//! `TypeSafe-Danger-Zone-Images: true` header for any request with images or a Box
//! question. Serializing a [`SystemOneRequest`](crate::SystemOneRequest) yourself produces
//! the JSON form, with each image base64-encoded.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use indexmap::IndexMap;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Content, Question, Questions, ValidationError};

/// Largest image file the API accepts: 16 MiB.
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Longest reference the API accepts, in bytes of UTF-8.
pub const MAX_REFERENCE_BYTES: usize = 1024;
/// Most bytes all references in one request may use together.
pub const MAX_TOTAL_REFERENCE_BYTES: usize = 65_536;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = b"\xff\xd8\xff";

/// The file format of an [`Image`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG. Animated PNGs are rejected by the API.
    Png,
    /// JPEG. A standard 8-bit baseline JPEG is the safest kind to send.
    Jpeg,
}

impl ImageFormat {
    /// The `format` value used on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// The MIME type of the format.
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    /// The format whose signature `bytes` start with.
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(PNG_MAGIC) {
            Some(Self::Png)
        } else if bytes.starts_with(JPEG_MAGIC) {
            Some(Self::Jpeg)
        } else {
            None
        }
    }
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ImageFormat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ImageFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            other => Err(D::Error::unknown_variant(other, &["png", "jpg", "jpeg"])),
        }
    }
}

/// An image file to send with a request: its bytes and their format.
///
/// The API accepts still PNG and JPEG images from 280 to 1568 pixels on each side, up to
/// 16 MiB. It removes metadata without applying the EXIF orientation, so rotate the pixels
/// upright before sending. See [Images](https://docs.typesafe.ai/images) for every limit.
///
/// [`SystemOneRequest::validate`](crate::SystemOneRequest::validate) checks the file size
/// and that the bytes start like a PNG or JPEG. The API checks the rest.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    format: ImageFormat,
    data: Vec<u8>,
}

impl Image {
    /// A PNG image.
    pub fn png(data: impl Into<Vec<u8>>) -> Self {
        Self::new(ImageFormat::Png, data)
    }

    /// A JPEG image.
    pub fn jpeg(data: impl Into<Vec<u8>>) -> Self {
        Self::new(ImageFormat::Jpeg, data)
    }

    /// An image in `format`.
    pub fn new(format: ImageFormat, data: impl Into<Vec<u8>>) -> Self {
        Self {
            format,
            data: data.into(),
        }
    }

    /// An image whose format is read from the file signature, or `None` when the bytes are
    /// neither PNG nor JPEG.
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> Option<Self> {
        let data = data.into();
        ImageFormat::detect(&data).map(|format| Self { format, data })
    }

    /// The image's format.
    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// The image file's bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The image file's bytes, taking ownership.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    fn check(&self, reference: &str) -> Result<(), ValidationError> {
        if self.data.len() > MAX_IMAGE_BYTES {
            return Err(ValidationError::ImageTooLarge {
                reference: reference.to_owned(),
                bytes: self.data.len(),
            });
        }
        if ImageFormat::detect(&self.data).is_none() {
            return Err(ValidationError::NotAnImage {
                reference: reference.to_owned(),
            });
        }
        Ok(())
    }
}

/// Shows the format and size, not the bytes.
impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("format", &self.format)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// Serialized as the API's base64 asset: `{"type": "image", "format", "data_base64"}`.
impl Serialize for Image {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireImage {
            r#type: Some("image".to_owned()),
            format: Some(self.format),
            data_base64: Some(BASE64.encode(&self.data)),
            part: None,
        }
        .serialize(serializer)
    }
}

/// Reads the base64 asset form. Multipart assets (`part`) have no bytes to read and are
/// rejected.
impl<'de> Deserialize<'de> for Image {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireImage::deserialize(deserializer)?;
        if wire.r#type.as_deref().is_some_and(|kind| kind != "image") {
            return Err(D::Error::custom("asset `type` must be \"image\""));
        }
        let (Some(format), Some(data)) = (wire.format, wire.data_base64) else {
            return Err(D::Error::custom(
                "an image asset needs `format` and `data_base64`",
            ));
        };
        let data = BASE64.decode(data).map_err(D::Error::custom)?;
        Ok(Self { format, data })
    }
}

#[derive(Serialize, Deserialize)]
struct WireImage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<ImageFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    part: Option<String>,
}

/// The reference string an image was added under, returned by [`Assets::add`].
///
/// Put it wherever the image belongs: as a JSON value (`json!({"photo": photo})`), as
/// content (`Content::from(photo)`), or inside text (`format!("... like this one: {photo}")`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageRef(String);

impl ImageRef {
    /// The reference string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ImageRef {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for ImageRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl From<ImageRef> for Content {
    fn from(reference: ImageRef) -> Self {
        Self::Text(reference.0)
    }
}

impl From<&ImageRef> for Content {
    fn from(reference: &ImageRef) -> Self {
        Self::Text(reference.0.clone())
    }
}

impl From<ImageRef> for serde_json::Value {
    fn from(reference: ImageRef) -> Self {
        Self::String(reference.0)
    }
}

/// The images of one request, keyed by reference string.
///
/// Every reference must appear somewhere in the request's state, instructions or criteria;
/// [`SystemOneRequest::validate`](crate::SystemOneRequest::validate) reports one that
/// doesn't. References are matched as plain text, also inside longer strings, so they must
/// not occur in your text by accident. [`Assets::add`] generates references that won't.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Assets {
    images: IndexMap<String, Image>,
}

impl Assets {
    /// No images.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `image` under a new random reference and returns the reference.
    pub fn add(&mut self, image: Image) -> ImageRef {
        loop {
            let reference = format!("img-{:032x}", fastrand::u128(..));
            if !self.images.contains_key(&reference) {
                self.images.insert(reference.clone(), image);
                return ImageRef(reference);
            }
        }
    }

    /// Adds `image` under a reference you choose, replacing any image already under it.
    ///
    /// Pick references that can't occur in your text by accident: a short one such as
    /// `cat` would also match inside `category`.
    pub fn insert(&mut self, reference: impl Into<String>, image: Image) -> ImageRef {
        let reference = reference.into();
        self.images.insert(reference.clone(), image);
        ImageRef(reference)
    }

    /// The image under `reference`.
    pub fn get(&self, reference: &str) -> Option<&Image> {
        self.images.get(reference)
    }

    /// References with their images, in the order they were added.
    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Image> {
        self.images.iter()
    }

    /// Number of images.
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Whether there are no images.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Checks the reference limits, the image files, and that each reference is used in
    /// `state` or `questions`.
    pub(crate) fn validate(
        &self,
        state: &Content,
        questions: &Questions,
    ) -> Result<(), ValidationError> {
        let mut total = 0;
        for (reference, image) in &self.images {
            if reference.trim().is_empty() {
                return Err(ValidationError::EmptyImageReference);
            }
            if reference.len() > MAX_REFERENCE_BYTES {
                return Err(ValidationError::ImageReferenceTooLong {
                    bytes: reference.len(),
                });
            }
            total += reference.len();
            image.check(reference)?;
        }
        if total > MAX_TOTAL_REFERENCE_BYTES {
            return Err(ValidationError::ImageReferencesTooLong { bytes: total });
        }

        let texts = request_texts(state, questions);
        for reference in self.images.keys() {
            if !texts.iter().any(|text| text.contains(reference.as_str())) {
                return Err(ValidationError::UnreferencedImage {
                    reference: reference.clone(),
                });
            }
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a Assets {
    type Item = (&'a String, &'a Image);
    type IntoIter = indexmap::map::Iter<'a, String, Image>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Every string value the API searches for references. Object keys are not replaced, so
/// they are left out.
fn request_texts<'a>(state: &'a Content, questions: &'a Questions) -> Vec<&'a str> {
    let mut texts = Vec::new();
    collect_content(state, &mut texts);
    for (_, question) in questions {
        match question {
            Question::Noul(noul) => {
                collect_option(noul.instructions.as_ref(), &mut texts);
                if let Some(criteria) = &noul.criteria {
                    collect_option(criteria.yes.as_ref(), &mut texts);
                    collect_option(criteria.no.as_ref(), &mut texts);
                }
            }
            Question::Choice(choice) => {
                collect_option(choice.instructions.as_ref(), &mut texts);
                for description in choice.criteria.values() {
                    collect_option(description.as_ref(), &mut texts);
                }
            }
            Question::Score(score) => {
                collect_option(score.instructions.as_ref(), &mut texts);
                for level in &score.criteria {
                    collect_content(level, &mut texts);
                }
            }
            Question::Box(question) => collect_option(question.instructions.as_ref(), &mut texts),
        }
    }
    texts
}

fn collect_option<'a>(content: Option<&'a Content>, texts: &mut Vec<&'a str>) {
    if let Some(content) = content {
        collect_content(content, texts);
    }
}

fn collect_content<'a>(content: &'a Content, texts: &mut Vec<&'a str>) {
    match content {
        Content::Text(text) => texts.push(text),
        Content::Object(object) => {
            for value in object.values() {
                collect_value(value, texts);
            }
        }
        Content::Array(items) => {
            for value in items {
                collect_value(value, texts);
            }
        }
    }
}

fn collect_value<'a>(value: &'a serde_json::Value, texts: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => texts.push(text),
        serde_json::Value::Object(object) => {
            for value in object.values() {
                collect_value(value, texts);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items {
                collect_value(value, texts);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{BoxQuestion, ChoiceQuestion, NoulQuestion, ScoreQuestion};

    const PNG: &[u8] = include_bytes!("../tests/fixtures/red-rectangle.png");

    fn noul(instructions: impl Into<Content>) -> Questions {
        let mut questions = Questions::new();
        questions.add("q", NoulQuestion::new(instructions));
        questions
    }

    #[test]
    fn detects_png_and_jpeg_from_their_signatures() {
        assert_eq!(ImageFormat::detect(PNG), Some(ImageFormat::Png));
        assert_eq!(
            ImageFormat::detect(b"\xff\xd8\xff\xe0rest"),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::detect(b"GIF89a"), None);
        assert!(Image::from_bytes(b"GIF89a".to_vec()).is_none());
        assert_eq!(
            Image::from_bytes(PNG.to_vec()).map(|image| image.format()),
            Some(ImageFormat::Png)
        );
    }

    #[test]
    fn serializes_images_as_base64_assets_and_reads_them_back() {
        let image = Image::png(PNG.to_vec());

        let wire = serde_json::to_value(&image).unwrap();

        assert_eq!(wire["type"], "image");
        assert_eq!(wire["format"], "png");
        assert_eq!(wire["data_base64"], BASE64.encode(PNG));
        assert_eq!(serde_json::from_value::<Image>(wire).unwrap(), image);
    }

    #[test]
    fn reads_jpeg_as_another_name_for_jpg() {
        let image: Image =
            serde_json::from_value(json!({"format": "jpeg", "data_base64": "/9j/"})).unwrap();

        assert_eq!(image.format(), ImageFormat::Jpeg);
        assert_eq!(image.data(), b"\xff\xd8\xff");
    }

    #[test]
    fn rejects_multipart_assets_when_deserializing_because_they_have_no_bytes() {
        let error =
            serde_json::from_value::<Image>(json!({"type": "image", "part": "photo"})).unwrap_err();

        assert!(error.to_string().contains("data_base64"), "{error}");
    }

    #[test]
    fn hides_image_bytes_in_debug_output() {
        let debug = format!("{:?}", Image::png(PNG.to_vec()));

        assert_eq!(
            debug,
            format!("Image {{ format: Png, bytes: {} }}", PNG.len())
        );
    }

    #[test]
    fn generates_distinct_references_that_serialize_as_strings() {
        let mut assets = Assets::new();

        let first = assets.add(Image::png(PNG.to_vec()));
        let second = assets.add(Image::png(PNG.to_vec()));

        assert_ne!(first, second);
        assert!(first.as_str().starts_with("img-"));
        assert_eq!(json!({"photo": first}), json!({"photo": first.as_str()}));
        assert_eq!(Content::from(&second), Content::from(second.as_str()));
        assert_eq!(assets.len(), 2);
    }

    #[test]
    fn accepts_references_in_state_instructions_and_criteria() {
        let mut assets = Assets::new();
        let in_state = assets.add(Image::png(PNG.to_vec()));
        let in_text = assets.add(Image::png(PNG.to_vec()));
        let in_option = assets.add(Image::png(PNG.to_vec()));
        let in_level = assets.add(Image::png(PNG.to_vec()));
        let in_box = assets.add(Image::png(PNG.to_vec()));
        let mut questions = Questions::new();
        questions.add(
            "same",
            NoulQuestion::new(format!("Is `frames[0].frame` the dog in {in_text}?")),
        );
        questions.add(
            "gesture",
            ChoiceQuestion::new("Which gesture?")
                .with_option("thumbs_up", json!({"examples": [in_option]}))
                .with_bare_option("none"),
        );
        questions.add(
            "open",
            ScoreQuestion::new(
                "How open?",
                [Content::from(in_level), Content::from("open")],
            ),
        );
        questions.add(
            "where",
            BoxQuestion::new(format!("the badge holder {in_box}")),
        );

        let state = Content::from(json!({"frames": [{"seconds_ago": 0, "frame": in_state}]}));

        assert_eq!(assets.validate(&state, &questions), Ok(()));
    }

    #[test]
    fn rejects_an_image_that_no_text_references() {
        let mut assets = Assets::new();
        let unused = assets.add(Image::png(PNG.to_vec()));

        assert_eq!(
            assets.validate(&Content::from("no images here"), &noul("Is it?")),
            Err(ValidationError::UnreferencedImage {
                reference: unused.to_string()
            })
        );
    }

    #[test]
    fn does_not_count_object_keys_as_references() {
        let mut assets = Assets::new();
        let reference = assets.insert("img-key", Image::png(PNG.to_vec()));
        let state = Content::from(json!({ reference.as_str(): "a label" }));

        assert!(matches!(
            assets.validate(&state, &noul("?")),
            Err(ValidationError::UnreferencedImage { .. })
        ));
    }

    #[test]
    fn rejects_references_outside_the_documented_limits() {
        let state = |reference: &str| Content::from(reference.to_owned());
        let png = || Image::png(PNG.to_vec());

        let mut blank = Assets::new();
        blank.insert(" ", png());
        assert_eq!(
            blank.validate(&state(" "), &noul("?")),
            Err(ValidationError::EmptyImageReference)
        );

        let long = "r".repeat(MAX_REFERENCE_BYTES + 1);
        let mut too_long = Assets::new();
        too_long.insert(long.clone(), png());
        assert_eq!(
            too_long.validate(&state(&long), &noul("?")),
            Err(ValidationError::ImageReferenceTooLong {
                bytes: MAX_REFERENCE_BYTES + 1
            })
        );

        let mut many = Assets::new();
        let references: Vec<String> = (0..65)
            .map(|i| format!("{i:02}{}", "r".repeat(MAX_REFERENCE_BYTES - 2)))
            .collect();
        for reference in &references {
            many.insert(reference.clone(), png());
        }
        assert_eq!(
            many.validate(&Content::from(json!(references)), &noul("?")),
            Err(ValidationError::ImageReferencesTooLong {
                bytes: 65 * MAX_REFERENCE_BYTES
            })
        );
    }

    #[test]
    fn rejects_files_that_are_not_png_or_jpeg_or_too_large() {
        let mut gif = Assets::new();
        let reference = gif.insert("img-gif", Image::png(b"GIF89a".to_vec()));
        assert_eq!(
            gif.validate(&Content::from(reference.as_str()), &noul("?")),
            Err(ValidationError::NotAnImage {
                reference: "img-gif".into()
            })
        );

        let mut huge_bytes = PNG.to_vec();
        huge_bytes.resize(MAX_IMAGE_BYTES + 1, 0);
        let mut huge = Assets::new();
        huge.insert("img-huge", Image::png(huge_bytes));
        assert_eq!(
            huge.validate(&Content::from("img-huge"), &noul("?")),
            Err(ValidationError::ImageTooLarge {
                reference: "img-huge".into(),
                bytes: MAX_IMAGE_BYTES + 1
            })
        );
    }
}
