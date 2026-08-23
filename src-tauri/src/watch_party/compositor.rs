use std::ffi::c_void;

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::{
    Fxc::D3DCompile, ID3DBlob, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows_capture::d3d11::create_d3d_device;
use windows_capture::encoder::DetachedFrame;
use windows_capture::settings::ColorFormat;

use super::layout::{CompositionPlan, SourcePlacement};

const SHADER: &[u8] = br#"
struct VOut { float4 position : SV_POSITION; float2 uv : TEXCOORD0; };
cbuffer Crop : register(b0) { float4 crop; };
VOut vs_main(uint id : SV_VertexID) {
  const float2 positions[4] = {
    float2(-1.0, 1.0), float2(1.0, 1.0),
    float2(-1.0, -1.0), float2(1.0, -1.0)
  };
  const float2 uvs[4] = {
    float2(0.0, 0.0), float2(1.0, 0.0),
    float2(0.0, 1.0), float2(1.0, 1.0)
  };
  VOut output; output.position = float4(positions[id], 0.0, 1.0); output.uv = crop.xy + uvs[id] * crop.zw; return output;
}
Texture2D image : register(t0); SamplerState image_sampler : register(s0);
float4 ps_main(VOut input) : SV_TARGET { return image.Sample(image_sampler, input.uv); }
"#;

#[derive(Clone, Debug)]
pub struct CpuFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub captured_qpc_100ns: i64,
    pub generation: u64,
}

struct SourceTexture {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    generation: u64,
}

pub struct GpuCompositor {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: DetachedFrame,
    target: ID3D11RenderTargetView,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    crop_buffer: ID3D11Buffer,
    main: Option<SourceTexture>,
    reaction: Option<SourceTexture>,
}

impl GpuCompositor {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let (device, context) = create_d3d_device()
            .map_err(|error| format!("Could not create the Watch Party D3D11 device: {error}"))?;
        let output =
            DetachedFrame::new_render_target(&device, &context, width, height, ColorFormat::Bgra8)
                .map_err(|error| {
                    format!("Could not create the Watch Party render target: {error}")
                })?;
        let mut target = None;
        unsafe {
            device
                .CreateRenderTargetView(output.as_raw_texture(), None, Some(&mut target))
                .map_err(|error| {
                    format!("Could not create the Watch Party target view: {error}")
                })?;
        }
        let vertex_blob = compile_shader(s!("vs_main"), s!("vs_4_0"))?;
        let pixel_blob = compile_shader(s!("ps_main"), s!("ps_4_0"))?;
        let mut vertex_shader = None;
        let mut pixel_shader = None;
        unsafe {
            device
                .CreateVertexShader(blob_bytes(&vertex_blob), None, Some(&mut vertex_shader))
                .map_err(|error| {
                    format!("Could not create the Watch Party vertex shader: {error}")
                })?;
            device
                .CreatePixelShader(blob_bytes(&pixel_blob), None, Some(&mut pixel_shader))
                .map_err(|error| {
                    format!("Could not create the Watch Party pixel shader: {error}")
                })?;
        }
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe {
            device
                .CreateSamplerState(&sampler_desc, Some(&mut sampler))
                .map_err(|error| format!("Could not create the Watch Party sampler: {error}"))?;
        }
        let crop_desc = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let mut crop_buffer = None;
        unsafe {
            device
                .CreateBuffer(&crop_desc, None, Some(&mut crop_buffer))
                .map_err(|error| {
                    format!("Could not create the Watch Party crop buffer: {error}")
                })?;
        }

        Ok(Self {
            device,
            context,
            output,
            target: target
                .ok_or_else(|| "D3D11 returned no Watch Party target view.".to_string())?,
            vertex_shader: vertex_shader
                .ok_or_else(|| "D3D11 returned no Watch Party vertex shader.".to_string())?,
            pixel_shader: pixel_shader
                .ok_or_else(|| "D3D11 returned no Watch Party pixel shader.".to_string())?,
            sampler: sampler.ok_or_else(|| "D3D11 returned no Watch Party sampler.".to_string())?,
            crop_buffer: crop_buffer
                .ok_or_else(|| "D3D11 returned no Watch Party crop buffer.".to_string())?,
            main: None,
            reaction: None,
        })
    }

    pub fn compose(
        &mut self,
        main: &CpuFrame,
        reaction: &CpuFrame,
        plan: CompositionPlan,
    ) -> Result<&DetachedFrame, String> {
        update_source(&self.device, &self.context, &mut self.main, main)?;
        update_source(&self.device, &self.context, &mut self.reaction, reaction)?;
        unsafe {
            self.context.OMSetRenderTargets(
                Some(&[Some(self.target.clone())]),
                None::<&ID3D11DepthStencilView>,
            );
            self.context
                .ClearRenderTargetView(&self.target, &[0.018, 0.020, 0.027, 1.0]);
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.context.VSSetShader(&self.vertex_shader, None);
            self.context.PSSetShader(&self.pixel_shader, None);
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }
        draw_source(
            &self.context,
            &self.crop_buffer,
            self.main.as_ref().unwrap(),
            plan.main,
        );
        draw_source(
            &self.context,
            &self.crop_buffer,
            self.reaction.as_ref().unwrap(),
            plan.reaction,
        );
        unsafe {
            self.context.PSSetShaderResources(0, Some(&[None]));
            self.context.Flush();
        }
        Ok(&self.output)
    }

    pub fn compose_participants(
        &mut self,
        main: &CpuFrame,
        reaction: &CpuFrame,
        main_placement: SourcePlacement,
        reactions: &[SourcePlacement],
    ) -> Result<&DetachedFrame, String> {
        update_source(&self.device, &self.context, &mut self.main, main)?;
        update_source(&self.device, &self.context, &mut self.reaction, reaction)?;
        unsafe {
            self.context.OMSetRenderTargets(
                Some(&[Some(self.target.clone())]),
                None::<&ID3D11DepthStencilView>,
            );
            self.context
                .ClearRenderTargetView(&self.target, &[0.018, 0.020, 0.027, 1.0]);
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.context.VSSetShader(&self.vertex_shader, None);
            self.context.PSSetShader(&self.pixel_shader, None);
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context
                .VSSetConstantBuffers(0, Some(&[Some(self.crop_buffer.clone())]));
        }
        draw_source(
            &self.context,
            &self.crop_buffer,
            self.main.as_ref().unwrap(),
            main_placement,
        );
        for placement in reactions {
            draw_source(
                &self.context,
                &self.crop_buffer,
                self.reaction.as_ref().unwrap(),
                *placement,
            );
        }
        unsafe {
            self.context.PSSetShaderResources(0, Some(&[None]));
            self.context.Flush();
        }
        Ok(&self.output)
    }
}

fn update_source(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    slot: &mut Option<SourceTexture>,
    frame: &CpuFrame,
) -> Result<(), String> {
    if frame.pixels.len() != frame.width as usize * frame.height as usize * 4 {
        return Err("A Watch Party source frame had invalid BGRA storage.".to_string());
    }
    if slot
        .as_ref()
        .is_none_or(|texture| texture.width != frame.width || texture.height != frame.height)
    {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: frame.width,
            Height: frame.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|error| {
                    format!("Could not allocate a Watch Party source texture: {error}")
                })?;
        }
        let texture = texture.ok_or_else(|| "D3D11 returned no source texture.".to_string())?;
        let mut view = None;
        unsafe {
            device
                .CreateShaderResourceView(&texture, None, Some(&mut view))
                .map_err(|error| format!("Could not create a Watch Party source view: {error}"))?;
        }
        *slot = Some(SourceTexture {
            texture,
            view: view.ok_or_else(|| "D3D11 returned no source view.".to_string())?,
            width: frame.width,
            height: frame.height,
            generation: 0,
        });
    }
    let source = slot.as_mut().unwrap();
    if source.generation != frame.generation {
        unsafe {
            context.UpdateSubresource(
                &source.texture,
                0,
                None,
                frame.pixels.as_ptr().cast::<c_void>(),
                frame.width * 4,
                frame.width * frame.height * 4,
            );
        }
        source.generation = frame.generation;
    }
    Ok(())
}

fn draw_source(
    context: &ID3D11DeviceContext,
    crop_buffer: &ID3D11Buffer,
    source: &SourceTexture,
    placement: SourcePlacement,
) {
    let destination = placement.destination;
    let viewport = D3D11_VIEWPORT {
        TopLeftX: destination.x,
        TopLeftY: destination.y,
        Width: destination.width,
        Height: destination.height,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let crop = [
        placement.source_uv.x,
        placement.source_uv.y,
        placement.source_uv.width,
        placement.source_uv.height,
    ];
    unsafe {
        context.UpdateSubresource(crop_buffer, 0, None, crop.as_ptr().cast::<c_void>(), 0, 0);
        context.VSSetConstantBuffers(0, Some(&[Some(crop_buffer.clone())]));
        context.RSSetViewports(Some(&[viewport]));
        context.PSSetShaderResources(0, Some(&[Some(source.view.clone())]));
        context.Draw(4, 0);
    }
}

fn compile_shader(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob, String> {
    let mut code = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None::<&windows::Win32::Graphics::Direct3D::ID3DInclude>,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    result.map_err(|error| {
        errors
            .as_ref()
            .map(|blob| unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    blob.GetBufferPointer().cast::<u8>(),
                    blob.GetBufferSize(),
                ))
                .into_owned()
            })
            .unwrap_or_else(|| format!("D3D shader compilation failed: {error}"))
    })?;
    code.ok_or_else(|| "D3D shader compilation returned no bytecode.".to_string())
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

unsafe impl Send for GpuCompositor {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch_party::layout::{composition_plan, RectF, SourcePlacement, WatchPartyLayout};

    #[test]
    fn compiles_shaders_and_gpu_composes_two_bgra_sources() {
        let mut compositor = GpuCompositor::new(320, 180).unwrap();
        let main = CpuFrame {
            pixels: vec![0x44; 4 * 4 * 4],
            width: 4,
            height: 4,
            captured_qpc_100ns: 1,
            generation: 1,
        };
        let reaction = CpuFrame {
            pixels: vec![0x99; 2 * 4 * 4],
            width: 2,
            height: 4,
            captured_qpc_100ns: 2,
            generation: 1,
        };
        let plan = composition_plan(
            WatchPartyLayout::ReactionsRight,
            320,
            180,
            (main.width, main.height),
            (reaction.width, reaction.height),
        )
        .unwrap();
        compositor.compose(&main, &reaction, plan).unwrap();

        let participant_crops = [
            SourcePlacement {
                destination: RectF {
                    x: 220.0,
                    y: 10.0,
                    width: 90.0,
                    height: 75.0,
                },
                source_uv: RectF {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 0.5,
                },
            },
            SourcePlacement {
                destination: RectF {
                    x: 220.0,
                    y: 95.0,
                    width: 90.0,
                    height: 75.0,
                },
                source_uv: RectF {
                    x: 0.0,
                    y: 0.5,
                    width: 1.0,
                    height: 0.5,
                },
            },
        ];
        compositor
            .compose_participants(&main, &reaction, plan.main, &participant_crops)
            .unwrap();
    }
}
