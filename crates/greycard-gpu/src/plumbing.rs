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
        compilation_options: Default::default(),
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
