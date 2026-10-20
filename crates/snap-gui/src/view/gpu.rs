use super::Stretch;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use snap_core::{ProductId, Tile, TileKey};
use std::collections::HashMap;
use std::sync::Arc;

/// Textures kept on the GPU; least-recently-drawn ones beyond this are dropped.
const MAX_TEXTURES: usize = 512;
const QUAD_BYTES: u64 = 6 * 4 * 4; // 6 vertices of (x, y, u, v) f32

pub struct TileTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    last_used: u64,
}

struct CanvasRes {
    uniforms: wgpu::Buffer,
    lut: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    /// One tile bind group per draw, rebuilt each frame.
    tile_groups: Vec<wgpu::BindGroup>,
}

pub struct GpuState {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    canvas_layout: wgpu::BindGroupLayout,
    tile_layout: wgpu::BindGroupLayout,
    textures: HashMap<TileKey, TileTexture>,
    canvases: HashMap<u64, CanvasRes>,
    frame: u64,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn f32_bytes(v: impl IntoIterator<Item = f32>) -> Vec<u8> {
    v.into_iter().flat_map(f32::to_ne_bytes).collect()
}

impl GpuState {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let device = render_state.device.clone();
        let canvas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1),
            ],
        });
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile"),
            entries: &[texture_entry(0), texture_entry(1), texture_entry(2)],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas"),
            bind_group_layouts: &[&canvas_layout, &tile_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_state.target_format,
                    // egui's target is premultiplied; NaN pixels output all-zero.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        GpuState {
            device,
            queue: render_state.queue.clone(),
            pipeline,
            canvas_layout,
            tile_layout,
            textures: HashMap::new(),
            canvases: HashMap::new(),
            frame: 0,
        }
    }

    /// Uploads `tile` as an R32Float texture of its exact (clipped) shape unless already resident.
    pub fn upload(&mut self, key: &TileKey, tile: &Tile, no_data: Option<f32>) -> &TileTexture {
        if !self.textures.contains_key(key) {
            let data = tile.to_f32();
            let (rows, cols) = data.dim();
            let bytes = f32_bytes(
                data.iter()
                    .map(|&v| if Some(v) == no_data { f32::NAN } else { v }),
            );
            let size = wgpu::Extent3d {
                width: cols as u32,
                height: rows as u32,
                depth_or_array_layers: 1,
            };
            self.insert(key, size, wgpu::TextureFormat::R32Float, &bytes);
        }
        self.touch(key)
    }

    /// Uploads a mask tile as R8Unorm (255 where the pixel is set) unless already resident.
    pub fn upload_mask(&mut self, key: &TileKey, tile: &Tile) -> &TileTexture {
        if !self.textures.contains_key(key) {
            let data = tile.to_f32();
            let (rows, cols) = data.dim();
            let bytes: Vec<u8> = data
                .iter()
                .map(|&v| if v > 0.5 { 255 } else { 0 })
                .collect();
            let size = wgpu::Extent3d {
                width: cols as u32,
                height: rows as u32,
                depth_or_array_layers: 1,
            };
            self.insert(key, size, wgpu::TextureFormat::R8Unorm, &bytes);
        }
        self.touch(key)
    }

    fn insert(
        &mut self,
        key: &TileKey,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        bytes: &[u8],
    ) {
        let texture = self.texture(size, format, bytes);
        let view = texture.create_view(&Default::default());
        self.textures.insert(
            key.clone(),
            TileTexture {
                _texture: texture,
                view,
                last_used: self.frame,
            },
        );
    }

    fn touch(&mut self, key: &TileKey) -> &TileTexture {
        let t = self.textures.get_mut(key).unwrap();
        t.last_used = self.frame;
        t
    }

    pub fn evict_band(&mut self, id: ProductId, band: &str) {
        self.textures
            .retain(|k, _| !(k.product == id && k.band == band));
    }

    pub fn evict_product(&mut self, id: ProductId) {
        self.textures.retain(|k, _| k.product != id);
    }

    fn texture(
        &self,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        bytes: &[u8],
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_texture(&texture, size, bytes);
        texture
    }

    fn write_texture(&self, texture: &wgpu::Texture, size: wgpu::Extent3d, bytes: &[u8]) {
        self.queue.write_texture(
            texture.as_image_copy(),
            bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(texture.format().block_copy_size(None).unwrap() * size.width),
                rows_per_image: Some(size.height),
            },
            size,
        );
    }

    fn evict_lru(&mut self) {
        if self.textures.len() <= MAX_TEXTURES {
            return;
        }
        let mut old: Vec<(u64, TileKey)> = self
            .textures
            .iter()
            .filter(|(_, t)| t.last_used < self.frame)
            .map(|(k, t)| (t.last_used, k.clone()))
            .collect();
        old.sort_unstable_by_key(|(f, _)| *f);
        let excess = self.textures.len() - MAX_TEXTURES;
        for (_, k) in old.into_iter().take(excess) {
            self.textures.remove(&k);
        }
    }

    fn ensure_canvas(&mut self, id: u64, quads: usize) {
        let need = QUAD_BYTES * quads.max(1) as u64;
        let stale = self
            .canvases
            .get(&id)
            .is_none_or(|c| c.vertices.size() < need);
        if stale {
            let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canvas uniforms"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let lut_size = wgpu::Extent3d {
                width: 256,
                height: 1,
                depth_or_array_layers: 1,
            };
            let lut = self.texture(lut_size, wgpu::TextureFormat::Rgba8Unorm, &[0; 1024]);
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("canvas"),
                layout: &self.canvas_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &lut.create_view(&Default::default()),
                        ),
                    },
                ],
            });
            let vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canvas quads"),
                size: need.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.canvases.insert(
                id,
                CanvasRes {
                    uniforms,
                    lut,
                    bind_group,
                    vertices,
                    tile_groups: Vec::new(),
                },
            );
        }
    }
}

pub struct TileDraw {
    /// Per channel; a single-band draw repeats its tile in all three slots.
    pub keys: [TileKey; 3],
    pub tiles: [Arc<Tile>; 3],
    pub no_data: [Option<f32>; 3],
    /// Screen rect in points.
    pub screen: egui::Rect,
    /// Part of the tile texture to show, in 0..1.
    pub uv: egui::Rect,
}

pub struct CanvasCallback {
    pub id: u64,
    pub frame: u64,
    pub stretch: [Stretch; 3],
    pub mode: u32,
    /// Mode 2: overlay colour (rgb) and opacity (a).
    pub color: [f32; 4],
    pub lut: [[u8; 4]; 256],
    pub draws: Vec<TileDraw>,
}

impl CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<GpuState>() else {
            return Vec::new();
        };
        gpu.frame = self.frame;
        for d in &self.draws {
            for c in 0..3 {
                if self.mode == 2 {
                    gpu.upload_mask(&d.keys[c], &d.tiles[c]);
                } else {
                    gpu.upload(&d.keys[c], &d.tiles[c], d.no_data[c]);
                }
            }
        }
        gpu.evict_lru();

        let [w, h] = screen
            .size_in_pixels
            .map(|v| v as f32 / screen.pixels_per_point);
        let ndc = |p: egui::Pos2| (p.x / w * 2.0 - 1.0, 1.0 - p.y / h * 2.0);
        let mut verts = Vec::with_capacity(self.draws.len() * 24);
        for d in &self.draws {
            let (s, t) = (d.screen, d.uv);
            for (p, uv) in [
                (s.left_top(), t.left_top()),
                (s.right_top(), t.right_top()),
                (s.left_bottom(), t.left_bottom()),
                (s.right_top(), t.right_top()),
                (s.right_bottom(), t.right_bottom()),
                (s.left_bottom(), t.left_bottom()),
            ] {
                let (x, y) = ndc(p);
                verts.extend([x, y, uv.x, uv.y]);
            }
        }
        let st = &self.stretch;
        let mut uniforms = f32_bytes([
            st[0].min,
            st[1].min,
            st[2].min,
            0.0,
            st[0].max,
            st[1].max,
            st[2].max,
            0.0,
            st[0].gamma,
            st[1].gamma,
            st[2].gamma,
        ]);
        uniforms.extend(self.mode.to_ne_bytes());
        uniforms.extend(f32_bytes(self.color));
        let lut: Vec<u8> = self.lut.concat();
        let lut_size = wgpu::Extent3d {
            width: 256,
            height: 1,
            depth_or_array_layers: 1,
        };

        gpu.ensure_canvas(self.id, self.draws.len());
        let groups: Vec<_> = self
            .draws
            .iter()
            .map(|d| {
                let views = d.keys.each_ref().map(|k| &gpu.textures[k].view);
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("tile"),
                    layout: &gpu.tile_layout,
                    entries: &[0, 1, 2].map(|i| wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(views[i]),
                    }),
                })
            })
            .collect();
        gpu.canvases.get_mut(&self.id).unwrap().tile_groups = groups;
        let c = &gpu.canvases[&self.id];
        gpu.queue.write_buffer(&c.uniforms, 0, &uniforms);
        gpu.queue.write_buffer(&c.vertices, 0, &f32_bytes(verts));
        gpu.write_texture(&c.lut, lut_size, &lut);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<GpuState>() else {
            return;
        };
        let Some(c) = gpu.canvases.get(&self.id) else {
            return;
        };
        // Vertices are in whole-screen NDC, not relative to the callback rect.
        let [w, h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &c.bind_group, &[]);
        pass.set_vertex_buffer(0, c.vertices.slice(..));
        for (i, g) in c.tile_groups.iter().enumerate() {
            pass.set_bind_group(1, g, &[]);
            let v = i as u32 * 6;
            pass.draw(v..v + 6, 0..1);
        }
    }
}
