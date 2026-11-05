//! In-memory transfer of an evaluated model between instances of the same web build.
//! The native B-rep and validated model records stay authoritative. Display buffers only
//! prime display caches; receiving a transaction never tessellates or evaluates a feature.

use super::*;
use crate::display_quality::DisplayQuality;
use serde_json::Value as Json;
use solidcraft_geom::tess::{EvalCurve, EvalSurface, FacePlan};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct ModelTransfer {
    records: Option<Json>,
    bodies: Vec<usize>,
    geometry: Vec<Geometry>,
    display: Vec<Display>,
    quality: String,
}

#[derive(Serialize, Deserialize)]
enum Geometry {
    Brep(#[serde(with = "serde_bytes")] Vec<u8>),
    Mesh { positions: Vec<Vec3>, triangles: Vec<[u32; 3]>, color: Option<[f32; 3]>, paint: Option<kernel::Paint> },
}

impl Geometry {
    fn encode(body: &Body) -> Result<Self> {
        Ok(match body.triangle_mesh() {
            Some((positions, triangles)) => {
                Self::Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), color: body.color(), paint: body.paint().cloned() }
            }
            None => Self::Brep(kernel::encode_native_body(body)?),
        })
    }

    fn decode(self) -> Result<Body> {
        match self {
            Self::Brep(bytes) => Ok(kernel::decode_native_body(&bytes)?),
            Self::Mesh { positions, triangles, color, paint } => {
                if color.is_some_and(|color| color.iter().any(|x| !x.is_finite())) {
                    return Err(bad("mesh color"));
                }
                Ok(kernel::mesh_body(&positions, &triangles)?.with_color(color).with_paint(paint))
            }
        }
    }
}

fn bad(detail: &str) -> DocError {
    DocError::Invalid(format!("worker model: {detail}"))
}

/// Names plus the exact evaluation key distinguish the caches without comparing triangles.
#[derive(Serialize, Deserialize, Hash, PartialEq, Eq)]
struct DisplayKey {
    key: u64,
    feature: u64,
    name: String,
    transform: [[u64; 4]; 4],
}

impl DisplayKey {
    fn of(body: &ModelBody) -> Self {
        Self { key: body.key, feature: body.feature, name: body.name.clone(), transform: body.source_transform.map(|row| row.map(f64::to_bits)) }
    }
}

#[derive(Serialize, Deserialize)]
struct MeshBuffer {
    mesh: Arc<Mesh>,
    // These transient display flags are deliberately absent from the persisted mesh format.
    display_only_edges: bool,
    smooth_edges: Vec<bool>,
    sharp_edges: Vec<bool>,
}

impl MeshBuffer {
    fn of(mesh: Arc<Mesh>) -> Self {
        Self { display_only_edges: mesh.display_only_edges, smooth_edges: mesh.smooth_edges.clone(), sharp_edges: mesh.sharp_edges.clone(), mesh }
    }

    fn restore(self) -> Result<Arc<Mesh>> {
        let mut mesh = self.mesh;
        if mesh.positions.len() != mesh.normals.len()
            || mesh.triangles.len() != mesh.tri_face.len()
            || mesh.positions.iter().chain(&mesh.normals).chain(mesh.edges.iter().flatten()).any(|p| !p.is_finite())
            || mesh.triangles.iter().flatten().any(|i| *i as usize >= mesh.positions.len())
        {
            return Err(bad("display buffers"));
        }
        let m = Arc::make_mut(&mut mesh);
        m.display_only_edges = self.display_only_edges;
        m.smooth_edges = self.smooth_edges;
        m.sharp_edges = self.sharp_edges;
        Ok(mesh)
    }
}

#[derive(Serialize, Deserialize)]
struct Display {
    key: DisplayKey,
    reference: MeshBuffer,
    plan: Option<Plan>,
    planned: Option<MeshBuffer>,
    bounds: solidcraft_geom::Aabb3,
    open: bool,
}

impl ModelTransfer {
    /// Called on the computing worker, after a command has completed.
    pub fn capture(model: &Model, doc: &Document, quality: DisplayQuality) -> Result<Self> {
        let saved = if doc.features.is_empty() { None } else { Some(model.saved(doc).ok_or_else(|| bad("the evaluation cannot be transferred"))?) };
        let mut geometry = Vec::new();
        let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut bodies = Vec::new();
        if let Some(saved) = &saved {
            for body in &saved.bodies {
                let encoded = Geometry::encode(body)?;
                // A shared B-rep record can carry different paint or construction metadata.
                // Only identical native bytes may share a wire entry. Mesh bodies keep their
                // own triangles even when they share an empty B-rep record.
                let duplicate = if let Geometry::Brep(bytes) = &encoded {
                    let mut hash = std::collections::hash_map::DefaultHasher::new();
                    bytes.hash(&mut hash);
                    let bucket = seen.entry(hash.finish()).or_default();
                    let duplicate = bucket.iter().copied().find(|i| matches!(geometry.get(*i), Some(Geometry::Brep(known)) if known == bytes));
                    if duplicate.is_none() {
                        bucket.push(geometry.len());
                    }
                    duplicate
                } else {
                    None
                };
                let index = match duplicate {
                    Some(index) => index,
                    None => {
                        let index = geometry.len();
                        geometry.push(encoded);
                        index
                    }
                };
                bodies.push(index);
            }
        }
        let state = model.state();
        let display = state
            .bodies
            .iter()
            .map(|body| Display {
                key: DisplayKey::of(body),
                reference: MeshBuffer::of(body.mesh()),
                plan: body.display_plan_at(quality).as_deref().map(Plan::of),
                planned: body.plan_mesh_at(quality).map(MeshBuffer::of),
                bounds: body.outline_bounds(),
                open: body.is_open(),
            })
            .collect();
        Ok(Self { records: saved.map(|saved| saved.records), bodies, geometry, display, quality: quality.id().into() })
    }

    /// Restores exact geometry and cached display buffers. Missing feature states are rejected
    /// rather than letting Model::adopt evaluate them on the receiving UI thread.
    pub fn restore(self, doc: &Document) -> Result<Model> {
        let Some(records) = self.records else {
            return if doc.features.is_empty() { Ok(Model::default()) } else { Err(bad("missing evaluation")) };
        };
        let features = records.get("features").and_then(Json::as_array).ok_or_else(|| bad("feature states"))?;
        if features.iter().any(|feature| feature.get("state").is_none()) {
            return Err(bad("incomplete evaluation"));
        }
        let quality = DisplayQuality::parse(&self.quality).ok_or_else(|| bad("display quality"))?;
        let geometry = self.geometry.into_iter().map(Geometry::decode).collect::<Result<Vec<_>>>()?;
        let bodies = self.bodies.into_iter().map(|i| geometry.get(i).cloned().ok_or_else(|| bad("body index"))).collect::<Result<Vec<_>>>()?;
        let model = Model::adopt(doc, &crate::SavedModel { records, bodies })?;
        let display = self
            .display
            .into_iter()
            .map(|cache| {
                Ok((
                    cache.key,
                    (
                        cache.reference.restore()?,
                        cache.plan.map(Plan::restore).transpose()?.map(Arc::new),
                        cache.planned.map(MeshBuffer::restore).transpose()?,
                        cache.bounds,
                        cache.open,
                    ),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        for result in &model.results {
            for body in &result.state.bodies {
                if let Some((reference, plan, planned, bounds, open)) = display.get(&DisplayKey::of(body)) {
                    let _ = body.mesh.set(reference.clone());
                    if let Some(cell) = body.plan.get(quality.index()) {
                        let _ = cell.set(plan.clone());
                    }
                    if let Some(cell) = body.plan_mesh.get(quality.index()) {
                        let _ = cell.set(planned.clone());
                    }
                    let _ = body.outline.set((body.body.clone(), *bounds));
                    let _ = body.surface.set((body.body.clone(), *open));
                }
            }
        }
        Ok(model)
    }
}

// These are transient transport records, not a change to the geometry or design formats.
#[derive(Serialize, Deserialize)]
enum Surface {
    Plane { origin: Vec3, du: Vec3, dv: Vec3 },
    Nurbs { degree: [usize; 2], knots: [Vec<f64>; 2], count: [usize; 2], ctrl: Vec<[f64; 4]>, closed: [bool; 2] },
    Revolved { degree: usize, knots: Vec<f64>, ctrl: Vec<[f64; 4]>, origin: Vec3, axis: Vec3, transform: [[f64; 4]; 3], swap: bool },
}

impl Surface {
    fn of(surface: &EvalSurface) -> Self {
        match surface {
            EvalSurface::Plane { origin, du, dv } => Self::Plane { origin: *origin, du: *du, dv: *dv },
            EvalSurface::Nurbs { degree, knots, count, ctrl, closed } => {
                Self::Nurbs { degree: *degree, knots: knots.clone(), count: *count, ctrl: ctrl.clone(), closed: *closed }
            }
            EvalSurface::Revolved { profile, origin, axis, transform, swap } => Self::Revolved {
                degree: profile.degree,
                knots: profile.knots.clone(),
                ctrl: profile.ctrl.clone(),
                origin: *origin,
                axis: *axis,
                transform: *transform,
                swap: *swap,
            },
        }
    }

    fn restore(self) -> EvalSurface {
        match self {
            Self::Plane { origin, du, dv } => EvalSurface::Plane { origin, du, dv },
            Self::Nurbs { degree, knots, count, ctrl, closed } => EvalSurface::Nurbs { degree, knots, count, ctrl, closed },
            Self::Revolved { degree, knots, ctrl, origin, axis, transform, swap } => {
                EvalSurface::Revolved { profile: EvalCurve { degree, knots, ctrl }, origin, axis, transform, swap }
            }
        }
    }

    fn valid(&self) -> bool {
        let curve = |degree: usize, knots: &[f64], count: usize| {
            degree <= solidcraft_geom::bspline::MAX_SPLINE_DEGREE
                && count > degree
                && knots.len() == count.saturating_add(degree).saturating_add(1)
                && knots.len() <= solidcraft_geom::tess::MAX_EVAL_KNOTS
                && knots.iter().all(|x| x.is_finite())
                && knots.windows(2).all(|w| w.first() <= w.get(1))
        };
        match self {
            Self::Plane { origin, du, dv } => origin.is_finite() && du.is_finite() && dv.is_finite(),
            Self::Nurbs { degree, knots, count, ctrl, .. } => {
                count.first().copied().zip(count.get(1).copied()).and_then(|(u, v)| u.checked_mul(v)) == Some(ctrl.len())
                    && ctrl.len() <= solidcraft_geom::tess::MAX_EVAL_POINTS
                    && ctrl.iter().flatten().all(|x| x.is_finite())
                    && degree.iter().zip(knots).zip(count).all(|((degree, knots), count)| curve(*degree, knots, *count))
            }
            Self::Revolved { degree, knots, ctrl, origin, axis, transform, .. } => {
                curve(*degree, knots, ctrl.len())
                    && ctrl.len() <= solidcraft_geom::tess::MAX_EVAL_POINTS
                    && ctrl.iter().flatten().chain(transform.iter().flatten()).all(|x| x.is_finite())
                    && origin.is_finite()
                    && axis.is_finite()
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Face {
    face: u32,
    surface: Option<u32>,
    uv: Vec<[f64; 2]>,
    fixed: Vec<Vec3>,
    normals: Vec<Vec3>,
    flip: bool,
    triangles: Vec<[u32; 3]>,
}

#[derive(Serialize, Deserialize)]
struct Plan {
    surfaces: Vec<Surface>,
    faces: Vec<Face>,
    edges: Vec<Vec<Vec3>>,
    edge_faces: Vec<Vec<u32>>,
}

impl Plan {
    fn of(plan: &TessPlan) -> Self {
        Self {
            surfaces: plan.surfaces.iter().map(Surface::of).collect(),
            faces: plan
                .faces
                .iter()
                .map(|f| Face {
                    face: f.face,
                    surface: f.surface,
                    uv: f.uv.clone(),
                    fixed: f.fixed.clone(),
                    normals: f.normals.clone(),
                    flip: f.flip,
                    triangles: f.triangles.clone(),
                })
                .collect(),
            edges: plan.edges.clone(),
            edge_faces: plan.edge_faces.clone(),
        }
    }

    fn restore(self) -> Result<TessPlan> {
        if self.surfaces.iter().any(|surface| !surface.valid())
            || self.edges.iter().flatten().any(|p| !p.is_finite())
            || self.faces.iter().any(|face| {
                face.surface.is_some_and(|i| i as usize >= self.surfaces.len())
                    || face.triangles.iter().flatten().any(|i| *i as usize >= face.uv.len())
                    || face.fixed.len() > face.uv.len()
                    || (face.surface.is_none() && face.normals.len() != face.uv.len())
                    || face.fixed.iter().chain(&face.normals).any(|p| !p.is_finite())
                    || face.uv.iter().flatten().any(|v| !v.is_finite())
            })
        {
            return Err(bad("display plan"));
        }
        Ok(TessPlan {
            surfaces: self.surfaces.into_iter().map(Surface::restore).collect(),
            faces: self
                .faces
                .into_iter()
                .map(|f| FacePlan {
                    face: f.face,
                    surface: f.surface,
                    uv: f.uv,
                    fixed: f.fixed,
                    normals: f.normals,
                    flip: f.flip,
                    triangles: f.triangles,
                })
                .collect(),
            edges: self.edges,
            edge_faces: self.edge_faces,
        })
    }
}
