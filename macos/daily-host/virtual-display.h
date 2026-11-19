// The virtual display classes of CoreGraphics. They are NOT public API: Apple ships no header for
// them and documents nothing (docs/m4-plan.md, 4.3 B). This file exists for one small trial
// (docs/m4-plan.md, 6.2 item 2); whether anything like it goes into the product is the person's
// decision, to be written in an ADR.
//
// Where the declarations come from: the Objective-C runtime's own description of the classes
// (class_copyPropertyList and class_copyMethodList, with their type encodings), read on macOS
// 26.5.1. No other project's code was read for them. Only what the trial uses is declared.
// Weakly imported: when a macOS has them no more, the program still starts (and says so).

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

__attribute__((weak_import))
@interface CGVirtualDisplayDescriptor : NSObject
@property(nonatomic, strong) NSString *name;
@property(nonatomic) unsigned int vendorID;
@property(nonatomic) unsigned int productID;
@property(nonatomic) unsigned int serialNum;
@property(nonatomic) unsigned int maxPixelsWide;
@property(nonatomic) unsigned int maxPixelsHigh;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) CGPoint redPrimary;
@property(nonatomic) CGPoint greenPrimary;
@property(nonatomic) CGPoint bluePrimary;
@property(nonatomic) CGPoint whitePoint;
@property(nonatomic, strong, nullable) dispatch_queue_t queue;
@end

__attribute__((weak_import))
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)width
                       height:(unsigned int)height
                  refreshRate:(double)refreshRate;
@property(nonatomic, readonly) unsigned int width;
@property(nonatomic, readonly) unsigned int height;
@property(nonatomic, readonly) double refreshRate;
@end

__attribute__((weak_import))
@interface CGVirtualDisplaySettings : NSObject
@property(nonatomic, strong) NSArray<CGVirtualDisplayMode *> *modes;
@property(nonatomic) unsigned int hiDPI;
@end

__attribute__((weak_import))
@interface CGVirtualDisplay : NSObject
- (nullable instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@property(nonatomic, readonly) CGDirectDisplayID displayID;
@end

NS_ASSUME_NONNULL_END
