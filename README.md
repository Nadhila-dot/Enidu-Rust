# Enidu's Daemon in Rust 
for more performance and a significatlly smaller memory footprint. 

>[!NOTE]
>Current version of Enidu is very heavy on go's concurrency model and tends to use 100x more memory. Thus development on a rust alternative is better. 

>[!IMPORTANT]
> Current implementation has reduced RAM usage from 12GB to 3GB, which is a 75% decrease. Cpu is about a 35% decrease. 

# Current layout

Job -> Worker -> Connect

Todos
- Use tracing for logs



Problems
- Takes time to stop
- When attacks happen the Webserver slows down
- The attacks are well simply too simple.
- auto scaling not added


>[!NOTE]
>Enidu is a part of Nadhi.dev, but all illegal actions done by enidu cannot be traced back to Nadhi.dev
