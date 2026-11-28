//! What every op here builds its pipelines and dispatches from: the
//! binding types, the layouts, and a uniform buffer written a slot
//! at a time and read through a dynamic offset.

pub(crate) fn storage_texture(
    format: wgpu::TextureFormat,
    access: wgpu::StorageTextureAccess,
) -> wgpu::BindingType {
    wgpu::BindingType::StorageTexture {
        access,
        format,
        view_dimension: wgpu::TextureViewDimension::D2,
    }
}

pub(crate) const SAMPLED: wgpu::BindingType = wgpu::BindingType::Texture {
    sample_type: wgpu::TextureSampleType::Float { filterable: false },
    view_dimension: wgpu::TextureViewDimension::D2,
    multisampled: false,
};

/// A picture in full floats ([`crate::Image`]) as an op reads it: three
/// planes of one texture, red, green, and blue, a layer each. There is
/// no three-channel float format, and four channels held a fourth that
/// nothing read: a quarter of every such picture, 400 MB at 102 MP.
pub(crate) const PICTURE: wgpu::BindingType = wgpu::BindingType::Texture {
    sample_type: wgpu::TextureSampleType::Float { filterable: false },
    view_dimension: wgpu::TextureViewDimension::D2Array,
    multisampled: false,
};

/// The layers of a picture in full floats.
pub(crate) const PICTURE_LAYERS: u32 = 3;

/// A picture in full floats as an op writes it.
pub(crate) const PICTURE_OUT: wgpu::BindingType = wgpu::BindingType::StorageTexture {
    access: wgpu::StorageTextureAccess::WriteOnly,
    format: wgpu::TextureFormat::R32Float,
    view_dimension: wgpu::TextureViewDimension::D2Array,
};

/// A texture for a picture in full floats, `width` by `height`, its
/// three planes.
pub(crate) fn picture_texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: PICTURE_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage,
        view_formats: &[],
    })
}

/// A view of a picture's texture as the ops bind it, its three planes.
pub(crate) fn picture_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

pub(crate) const STORAGE_BUFFER: wgpu::BindingType = wgpu::BindingType::Buffer {
    ty: wgpu::BufferBindingType::Storage { read_only: false },
    has_dynamic_offset: false,
    min_binding_size: None,
};

pub(crate) const STORAGE_BUFFER_READ: wgpu::BindingType = wgpu::BindingType::Buffer {
    ty: wgpu::BufferBindingType::Storage { read_only: true },
    has_dynamic_offset: false,
    min_binding_size: None,
};

pub(crate) fn uniform(dynamic: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: dynamic,
        min_binding_size: None,
    }
}

/// A layout of `types` at bindings 0, 1, 2 and so on.
pub(crate) fn layout(
    device: &wgpu::Device,
    label: &str,
    types: &[wgpu::BindingType],
) -> wgpu::BindGroupLayout {
    let at: Vec<(u32, wgpu::BindingType)> = types
        .iter()
        .enumerate()
        .map(|(i, ty)| (i as u32, *ty))
        .collect();
    layout_at(device, label, &at)
}

/// A layout of the given bindings, which need not be contiguous: a
/// shader whose kernels use different subsets of its bindings can
/// have a layout per subset, each under the device's limits.
pub(crate) fn layout_at(
    device: &wgpu::Device,
    label: &str,
    bindings: &[(u32, wgpu::BindingType)],
) -> wgpu::BindGroupLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = bindings
        .iter()
        .map(|&(binding, ty)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

/// A compute pipeline for `entry`, its workgroup memory left as it
/// comes rather than zeroed first.
///
/// wgpu zeroes a kernel's `var<workgroup>` arrays at the start of every
/// workgroup unless told not to. On Metal that clearing cost more than
/// the kernels: the local contrast's column pass, whose array is 2304
/// floats, took about 145 ms at 24 MP with it and 3.5 ms without, and the
/// whole local contrast went from 0.98 s to 0.08 s on an M5 (§480).
/// It is safe only because every kernel here writes each element of
/// its workgroup memory before any invocation reads it, behind a
/// barrier: the box passes and the sharpen's two-stage blur load their
/// whole window first, the reductions have every invocation store its
/// slot, the CA vote's sites write their terms before one invocation
/// reads them, and a flag is set by one invocation ahead of
/// `workgroupUniformLoad`. A new kernel
/// must keep to that; one that reads an element it did not write gets
/// whatever the last workgroup left there.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    })
}

/// A uniform slot a dispatch reads through its dynamic offset.
pub(crate) const SLOT: u64 = 256;

/// The uniform slots a run writes, one per distinct dispatch setup.
pub(crate) struct Uniforms {
    pub(crate) buffer: wgpu::Buffer,
    next: std::cell::Cell<u64>,
    capacity: u64,
}

impl Uniforms {
    pub(crate) fn new(device: &wgpu::Device, label: &str, slots: u64) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: slots * SLOT,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            next: std::cell::Cell::new(0),
            capacity: slots,
        }
    }

    /// Write a slot; its dynamic offset.
    pub(crate) fn push<T: bytemuck::Pod>(&self, queue: &wgpu::Queue, params: &T) -> u32 {
        let slot = self.next.get();
        assert!(slot < self.capacity, "more uniform slots than planned");
        assert!(std::mem::size_of::<T>() as u64 <= SLOT);
        self.next.set(slot + 1);
        queue.write_buffer(&self.buffer, slot * SLOT, bytemuck::bytes_of(params));
        (slot * SLOT) as u32
    }

    /// The binding of one slot, for a dynamic offset to pick.
    pub(crate) fn binding(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: 0,
            size: Some(std::num::NonZeroU64::new(SLOT).unwrap()),
        })
    }
}

pub(crate) fn groups(n: u32, size: u32) -> u32 {
    n.div_ceil(size)
}

/// The bytes a single-sampled, single-level texture takes, from its
/// size and format (as a driver lays it out, give or take its own
/// padding).
pub(crate) fn texture_bytes(texture: &wgpu::Texture) -> u64 {
    let per_texel = texture
        .format()
        .block_copy_size(Some(wgpu::TextureAspect::All))
        .unwrap_or(0);
    u64::from(texture.width())
        * u64::from(texture.height())
        * u64::from(texture.depth_or_array_layers())
        * u64::from(per_texel)
}
