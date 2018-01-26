#![feature(repr_align)]
#![feature(attr_literals)]

extern crate cgmath;
extern crate tobj;
extern crate winit;

#[macro_use]
extern crate vulkano;
#[macro_use]
extern crate vulkano_shader_derive;
extern crate vulkano_text;
extern crate vulkano_win;

mod gl_types;
mod graphics;
mod tracer;
mod fps_counter;
mod camera;
mod cs;
mod scene;
mod input;
mod event_manager;

use vulkano::sync::GpuFuture;
use vulkano_win::VkSurfaceBuild;

use graphics::GraphicsPart;
use tracer::ComputePart;
use fps_counter::FPSCounter;
use event_manager::EventManager;

use std::sync::Arc;
use std::path::Path;

#[cfg(debug_assertions)]
fn message_types() -> vulkano::instance::debug::MessageTypes {
    vulkano::instance::debug::MessageTypes {
        error: true,
        warning: true,
        performance_warning: true,
        information: true,
        debug: true,
    }
}

#[cfg(not(debug_assertions))]
fn message_types() -> vulkano::instance::debug::MessageTypes {
    vulkano::instance::debug::MessageTypes {
        error: true,
        warning: true,
        performance_warning: true,
        information: false,
        debug: false,
    }
}

fn get_layers<'a>(desired_layers: Vec<&'a str>) -> Vec<&'a str> {
    let available_layers: Vec<_> = vulkano::instance::layers_list().unwrap().collect();
    println!("Available layers:");
    for l in &available_layers {
        println!("\t{}", l.name());
    }
    desired_layers
        .into_iter()
        .filter(|&l| available_layers.iter().any(|li| li.name() == l))
        .collect()
}

fn print_message_callback(msg: &vulkano::instance::debug::Message) {
    let ty = if msg.ty.error {
        "error"
    } else if msg.ty.warning {
        "warning"
    } else if msg.ty.performance_warning {
        "performance_warning"
    } else if msg.ty.information {
        "information"
    } else if msg.ty.debug {
        "debug"
    } else {
        panic!("no-impl");
    };
    println!("{} [{}] : {}", msg.layer_prefix, ty, msg.description);
}

struct Vulkan<'a> {
    physical: vulkano::instance::PhysicalDevice<'a>,
    device: Arc<vulkano::device::Device>,
    queue: Arc<vulkano::device::Queue>,
}

impl<'a> Vulkan<'a> {
    fn new<P>(instance: &Arc<vulkano::instance::Instance>, predicate: P) -> Vulkan
    where
        for<'r> P: FnMut(&'r vulkano::instance::QueueFamily) -> bool,
    {
        let physical = vulkano::instance::PhysicalDevice::enumerate(instance)
            .next()
            .expect("no device available");
        println!(
            "Using device: {} (type: {:?})",
            physical.name(),
            physical.ty()
        );
        let queue = physical
            .queue_families()
            .find(predicate)
            .expect("couldn't find a graphical queue family");
        let device_ext = vulkano::device::DeviceExtensions {
            khr_swapchain: true,
            ..vulkano::device::DeviceExtensions::none()
        };
        let (device, mut queues) = vulkano::device::Device::new(
            physical,
            physical.supported_features(),
            &device_ext,
            [(queue, 0.5)].iter().cloned(),
        ).expect("failed to create device");
        let queue = queues.next().unwrap();

        Vulkan {
            physical,
            device,
            queue,
        }
    }
}

fn main() {
    let extensions = vulkano::instance::InstanceExtensions {
        ext_debug_report: true,
        ..vulkano_win::required_extensions()
    };
    let layers = get_layers(vec!["VK_LAYER_LUNARG_standard_validation"]);
    println!("Using layers: {:?}", layers);
    let instance = vulkano::instance::Instance::new(None, &extensions, &layers)
        .expect("failed to create instance");

    let _debug_callback = vulkano::instance::debug::DebugCallback::new(
        &instance,
        message_types(),
        print_message_callback,
    ).ok();

    let mut events_loop = winit::EventsLoop::new();
    let window = winit::WindowBuilder::new()
        .with_dimensions(600, 600)
        .build_vk_surface(&events_loop, instance.clone())
        .unwrap();
    window.window().set_cursor(winit::MouseCursor::NoneCursor);

    let Vulkan {
        device,
        queue,
        physical,
    } = Vulkan::new(&instance, |&q| {
        q.supports_graphics() && window.surface().is_supported(q).unwrap_or(false)
    });

    let model_path = std::env::args().nth(1).expect("no model passed");
    let (scene_buffers, load_future) =
        scene::ModelBuffers::from_obj(Path::new(&model_path), device.clone(), queue.clone())
            .expect("failed to load model");

    let mut event_manager = EventManager::new();
    let mut fps_counter = FPSCounter::new(fps_counter::Duration::milliseconds(100));
    let mut camera = camera::Camera::new([40.0, 40.0]);

    let mut graphics = GraphicsPart::new(device.clone(), &window, physical.clone(), queue.clone());
    let mut compute = ComputePart::new(&device, graphics.texture.clone(), scene_buffers).unwrap();

    let uniform_buffer =
        vulkano::buffer::CpuBufferPool::<cs::ty::Constants>::uniform_buffer(device.clone());

    let mut previous_frame_end = load_future;
    loop {
        previous_frame_end.cleanup_finished();
        fps_counter.end_frame();

        if graphics.recreate_swapchain(&window) {
            continue;
        }

        graphics.recreate_framebuffers();

        let (image_num, aquire_future) = match graphics.acquire_next_image() {
            Ok(r) => r,
            Err(vulkano::swapchain::AcquireError::OutOfDate) => {
                continue;
            }
            Err(err) => panic!("{:?}", err),
        };

        let uniform = Arc::new(
            uniform_buffer
                .next(cs::ty::Constants {
                    camera: camera.gpu_camera::<cs::ty::Camera>(),
                })
                .expect("failed to create uniform buffer"),
        );

        let cb = {
            let mut cbb =
                vulkano::command_buffer::AutoCommandBufferBuilder::primary_one_time_submit(
                    device.clone(),
                    queue.family(),
                ).unwrap();

            cbb = compute.render(cbb, graphics.dimensions, uniform);
            cbb = graphics.draw(cbb, image_num);

            cbb.build().unwrap()
        };

        let future = previous_frame_end
            .join(aquire_future)
            .then_execute(queue.clone(), cb)
            .unwrap()
            .then_swapchain_present(queue.clone(), graphics.swapchain.clone(), image_num)
            .then_signal_fence_and_flush()
            .unwrap();
        previous_frame_end = Box::new(future) as Box<_>;

        let render_time = fps_counter.render_time();
        graphics.queue_text(
            10.0,
            20.0,
            20.0,
            &format!(
                "Using device: {}\nRender time: {} ms ({} FPS)\nCamera: {}",
                physical.name(),
                render_time,
                fps_counter.current_fps(),
                camera
            ),
        );

        events_loop.poll_events(|ev| event_manager.process_event(ev));
        camera.process_keyboard_input(&event_manager.keyboard, render_time as f32 / 1000.0);
        camera.process_mouse_input(event_manager.mouse.fetch_mouse_delta());
        graphics.recreate_swapchain = event_manager.recreate_swapchain();
        if event_manager.done() {
            return;
        }
    }
}
